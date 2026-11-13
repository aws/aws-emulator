// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-endusermessaging-endusermessaging service.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

/// The service's own ARN namespace / sigv4 signing name, as the Smithy model's
/// `aws.api#service` trait declares it (`arnNamespace`/`endpointPrefix`
/// `end-user-messaging`).
pub const ARN_NAMESPACE: &str = "end-user-messaging";

/// The region used when a request carries no SigV4 credential scope to read one
/// from (an anonymous, host-routed request). Every generated emulator defaults
/// here.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `brand-profile` resource segment of a brand-profile ARN.
const BRAND_PROFILE_RESOURCE: &str = "brand-profile";

/// One brand-profile attribute, keyed in [`BrandProfile::attributes`] by its
/// name (`BrandProfileAttributeName` is unique within a profile).
///
/// The attribute operations are still scaffolded, so nothing writes this yet;
/// the map it lives in is what `attributesCreated` counts.
#[derive(Debug, Clone)]
pub struct BrandProfileAttribute {
    /// `attributeName` — unique within the owning brand profile.
    pub name: String,
    /// `attributeType`, one of [`crate::ops::types::BRAND_PROFILE_ATTRIBUTE_TYPE_VALUES`].
    pub attribute_type: String,
    /// `attributeValue` for a `TEXT` attribute; `None` for uploaded media.
    pub value: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

/// A persisted brand profile: everything the brand-profile operations report.
#[derive(Debug, Clone)]
pub struct BrandProfile {
    /// The minted `brandProfileId` (`bp-` + 17 chars, per `BrandProfileIdOrArn`).
    pub id: String,
    /// `brandProfileArn`, built once at create time from the request's region.
    pub arn: String,
    pub name: String,
    /// `status`, one of [`crate::ops::types::STATUS_VALUES`]. A profile AWS
    /// returns from `CreateBrandProfile` is already `ACTIVE`.
    pub status: String,
    pub deletion_protection_enabled: bool,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// `tags` as supplied at create time; the tagging operations are scaffolded.
    pub tags: Vec<(String, String)>,
    /// The profile's attributes, empty for a freshly created profile.
    pub attributes: BTreeMap<String, BrandProfileAttribute>,
}

/// The most brand profiles this emulator stores for one account.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateBrandProfile` models
/// `ServiceQuotaExceededException`, which is the refusal once the store is full.
pub const MAX_BRAND_PROFILES: usize = 100;

/// The most `clientToken` entries kept for `CreateBrandProfile` idempotency.
///
/// At most one token is recorded per stored profile and a token is dropped with
/// the profile it minted, so this cap is never the binding one in practice; it is
/// enforced anyway, by evicting the oldest token, so the map cannot accumulate
/// caller-chosen strings independently of the profiles they name.
pub const MAX_CREATE_TOKENS: usize = MAX_BRAND_PROFILES;

/// Per-service state for com-amazonaws-endusermessaging-endusermessaging. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEndusermessagingEndusermessagingState {
    /// Every brand profile, keyed by `brandProfileId`. A `BTreeMap` so
    /// `ListBrandProfiles` pages in a stable, resumable order. Bounded by
    /// [`MAX_BRAND_PROFILES`].
    pub brand_profiles: BTreeMap<String, BrandProfile>,
    /// `CreateBrandProfile` idempotency: `clientToken` -> `brandProfileId`.
    /// Bounded by [`MAX_CREATE_TOKENS`]; write it through
    /// [`Self::record_create_token`], never directly.
    pub create_tokens: HashMap<String, String>,
    /// The order [`Self::create_tokens`] was written in, so the cap evicts the
    /// oldest token rather than an arbitrary one.
    create_token_order: VecDeque<String>,
    /// Per-state random-ish prefix for minted ids, set on the first mint.
    id_seed: u64,
    /// Monotonic counter making each minted id unique within this state.
    next_id: u64,
}

impl ComAmazonawsEndusermessagingEndusermessagingState {
    /// Mint a fresh, unused `brandProfileId`.
    ///
    /// The shape is the one `BrandProfileIdOrArn` documents — a `bp-` prefix
    /// followed by 17 lowercase alphanumerics — and it satisfies that shape's
    /// `^[A-Za-z0-9_:/-]+$` pattern. The clock-derived seed keeps ids from
    /// different runs apart; the counter keeps ids within one run unique, and
    /// the loop re-draws rather than ever handing back an id already in use.
    pub fn mint_brand_profile_id(&mut self) -> String {
        if self.id_seed == 0 {
            // 36 bits of the current nanosecond clock -> 9 hex digits.
            self.id_seed = (nanos_since_epoch() as u64) & 0x0000_000F_FFFF_FFFF;
        }
        loop {
            self.next_id = self.next_id.wrapping_add(1);
            let id = format!("bp-{:09x}{:08x}", self.id_seed, self.next_id & 0xFFFF_FFFF);
            if !self.brand_profiles.contains_key(&id) {
                return id;
            }
        }
    }

    /// Record `token` as having minted `brand_profile_id`, evicting the oldest
    /// token first if that would push the map past [`MAX_CREATE_TOKENS`].
    pub fn record_create_token(&mut self, token: String, brand_profile_id: &str) {
        while self.create_tokens.len() >= MAX_CREATE_TOKENS {
            match self.create_token_order.pop_front() {
                Some(oldest) => {
                    self.create_tokens.remove(&oldest);
                }
                // The order queue and the map are written together, so this is
                // unreachable; clearing rather than looping forever keeps the cap
                // an invariant even if they ever drift.
                None => {
                    self.create_tokens.clear();
                    break;
                }
            }
        }
        if self
            .create_tokens
            .insert(token.clone(), brand_profile_id.to_string())
            .is_none()
        {
            self.create_token_order.push_back(token);
        }
    }

    /// Drop every idempotency token that minted `brand_profile_id`, so a token
    /// never outlives the profile it names.
    pub fn forget_create_tokens_for(&mut self, brand_profile_id: &str) {
        self.create_tokens.retain(|_, minted| minted != brand_profile_id);
        let live = &self.create_tokens;
        self.create_token_order.retain(|token| live.contains_key(token));
    }

    /// Resolve a `BrandProfileIdOrArn` to the id of a stored profile.
    ///
    /// The shape accepts either a bare id or the profile's full ARN, so every
    /// operation taking one resolves both; an unknown value answers `None`, which
    /// the caller renders as `ResourceNotFoundException`.
    pub fn resolve_brand_profile_id(&self, id_or_arn: &str) -> Option<&str> {
        if let Some((id, _)) = self.brand_profiles.get_key_value(id_or_arn) {
            return Some(id.as_str());
        }
        let id = brand_profile_id_from_arn(id_or_arn)?;
        self.brand_profiles.get_key_value(id).map(|(id, _)| id.as_str())
    }
}

/// The id named by a brand-profile ARN
/// (`arn:<partition>:end-user-messaging:<region>:<account>:brand-profile/<id>`),
/// or `None` for anything that is not one.
fn brand_profile_id_from_arn(candidate: &str) -> Option<&str> {
    let mut fields = candidate.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some(ARN_NAMESPACE), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    let id = resource.strip_prefix(BRAND_PROFILE_RESOURCE)?.strip_prefix('/')?;
    (!id.is_empty()).then_some(id)
}

/// The ARN of a brand profile in `region`.
pub fn brand_profile_arn(region: &str, brand_profile_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{BRAND_PROFILE_RESOURCE}/{brand_profile_id}"),
    )
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries
/// no SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A stored instant as the modelled `Timestamp` the wire carries.
///
/// Built from whole seconds plus sub-second nanoseconds so a stored instant keeps
/// the resolution `SystemTime::now()` gave it; going through an `f64` of epoch
/// seconds would quantise it to roughly a quarter of a microsecond.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// The message for the one failure that is not about the request: this service was
/// mounted on an engine that holds no store for it.
pub const NO_STORE: &str =
    "the com-amazonaws-endusermessaging-endusermessaging store is not mounted on this engine";

/// `status` of a brand profile this emulator creates — a member of
/// `crate::ops::types::STATUS_VALUES`.
pub const ACTIVE_STATUS: &str = "ACTIVE";

/// The `resourceType` a brand-profile error reports: the hyphenated resource
/// vocabulary this API names the resource by, the same spelling it takes in a
/// brand-profile ARN. `ConflictException`/`ResourceNotFoundException` model
/// `resourceType` as a free `String`, not as the `JobResourceType` enum, so the
/// resource segment — not that enum's `BRAND_PROFILE` — is the token reported
/// here and quoted in the refusal messages below.
pub const BRAND_PROFILE_RESOURCE_TYPE: &str = BRAND_PROFILE_RESOURCE;

/// The conflict `Reason` token for a request refused because the resource it
/// names has deletion protection switched on.
pub const DELETION_PROTECTION_ENABLED_REASON: &str = "DELETION_PROTECTION_ENABLED";

/// `ResourceNotFoundException` for an identifier that names no stored resource of
/// `resource_type`.
///
/// The message is this error class's fixed template; its only variable parts are
/// the resource type and the identifier, which is the value the request gave.
pub fn resource_not_found(
    resource_type: &str,
    resource_id: &str,
) -> crate::ops::types::ResourceNotFoundException {
    crate::ops::types::ResourceNotFoundException {
        message: format!(
            "Resource not found - ResourceType=\"{resource_type}\" ResourceId=\"{resource_id}\""
        ),
        resource_id: Some(resource_id.to_string()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// `ConflictException` for a request the current state of `resource_id` forbids.
///
/// The message is this error class's fixed template: the quoted `Reason` token
/// names the state that caused the conflict and the quoted `ResourceType` the
/// resource's type; no request-supplied value appears in it, so the identifier is
/// reported in the `resourceId` member alone.
pub fn resource_conflict(
    reason: &str,
    resource_type: &str,
    resource_id: &str,
) -> crate::ops::types::ConflictException {
    crate::ops::types::ConflictException {
        message: format!("Conflict occurred - Reason=\"{reason}\" ResourceType=\"{resource_type}\""),
        resource_id: Some(resource_id.to_string()),
        resource_type: Some(resource_type.to_string()),
    }
}

/// `ResourceNotFoundException` for a `BrandProfileIdOrArn` that names no stored
/// profile. The message is built from the request, so it names the value given.
pub fn brand_profile_not_found(id_or_arn: &str) -> crate::ops::types::ResourceNotFoundException {
    resource_not_found(BRAND_PROFILE_RESOURCE_TYPE, id_or_arn)
}

/// `ConflictException` for a brand-profile request refused by that profile's
/// deletion protection.
pub fn brand_profile_deletion_protection_conflict(
    brand_profile_id: &str,
) -> crate::ops::types::ConflictException {
    resource_conflict(
        DELETION_PROTECTION_ENABLED_REASON,
        BRAND_PROFILE_RESOURCE_TYPE,
        brand_profile_id,
    )
}

fn nanos_since_epoch() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEndusermessagingEndusermessagingState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEndusermessagingEndusermessagingState>)
}
