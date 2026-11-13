// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-rolesanywhere-rolesanywhere service.
//!
//! The trust-anchor, profile and certificate-revocation-list lifecycles
//! (`CreateTrustAnchor`, `UpdateTrustAnchor`, `GetTrustAnchor`, `ListTrustAnchors`,
//! `DeleteTrustAnchor`, `DisableTrustAnchor`, `EnableTrustAnchor`, `PutNotificationSettings`,
//! `ResetNotificationSettings`,
//! `CreateProfile`, `GetProfile`, `ListProfiles`, `DisableProfile`, `DeleteProfile`,
//! `PutAttributeMapping`, `DeleteAttributeMapping`, `ImportCrl`,
//! `GetCrl`, `ListCrls`) are backed entirely by the maps below, so a write is visible to the next
//! request on the same engine. The record types and their projections live here rather
//! than in a handler because several operations reach each of them.
//!
//! Every one of those resources is taggable, and each keeps its own tag list on its
//! record, so the ARN-keyed tag operations (`TagResource`, `UntagResource`,
//! `ListTagsForResource`) resolve
//! an ARN against the records themselves rather than against a second tag table that
//! could disagree with them.
//!
//! The SUBJECT half — the audit record of each temporary-credential request the native
//! session plane serves, which `GetSubject` reads back — lives in [`subjects`], beside the
//! writer and the reader of that map.

/// The subjects the native session plane records: one record per certificate identity that
/// has asked this account for credentials.
pub mod subjects;

pub use subjects::*;

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AttributeMapping, CrlDetail, MappingRule, NotificationSetting, NotificationSettingDetail,
    NotificationSettingKey, ProfileDetail, Source, SourceData, Tag, TrustAnchorDetail,
    NOTIFICATION_EVENT_VALUES, TRUST_ANCHOR_TYPE_VALUES,
};

/// The ARN service namespace (`OPERATIONS.md`: "ARN namespace: `rolesanywhere`").
pub const ARN_NAMESPACE: &str = "rolesanywhere";

/// The region an unsigned request is attributed to, when no SigV4 credential scope
/// names one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `ProfileDetail$durationSeconds` when the create request omits it: "In requests, if
/// this value is not provided, the default value will be 3600."
/// (`smithy_query(query="shape", shape="ProfileDetail")`, member `durationSeconds`).
pub const DEFAULT_PROFILE_DURATION_SECONDS: i32 = 3600;

/// The largest page the `@paginated` list operations serve, and the page size a request
/// that omits `pageSize` gets.
///
/// `ListRequest$pageSize` carries no `@range` in the model, so this ceiling is this
/// emulator's own page bound rather than a published limit. It only bounds ONE
/// response; a caller that wants more pages the rest with `nextToken`.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The most trust anchors, the most profiles, and the most certificate revocation
/// lists this emulator holds.
///
/// Every store lives for the process's lifetime and grows only on a create, and none of
/// these resources can be given a TTL or evicted: a `GetTrustAnchor` / `GetProfile` /
/// `GetCrl` has to keep answering with what its create returned, so dropping a record
/// would be a divergence rather than a cleanup. A maximum entry count, refused on the create path,
/// is therefore the bound available here — the same construct this workspace's other
/// services use (`MAX_CHANNELS`, `MAX_STORED_CASES`). It sits far above any realistic
/// client workflow, so a caller never meets it by accident. None of these operations
/// models a quota error, so the refusal is their modelled `ValidationException`.
pub const MAX_TRUST_ANCHORS: usize = 10_000;
/// See [`MAX_TRUST_ANCHORS`].
pub const MAX_PROFILES: usize = 10_000;
/// See [`MAX_TRUST_ANCHORS`]. A CRL record also holds the imported `crlData`, whose own
/// size the modelled `Blob` `@length(1, 300000)` already caps per request, so this count
/// is what bounds the PRODUCT — the bytes every stored CRL can add up to.
pub const MAX_CRLS: usize = 1_000;

/// The most tags one resource holds at once.
///
/// AWS's documented tag restrictions bound a resource's tag set at 50 tags, with keys of
/// at most 128 characters and values of at most 256 — the same two lengths this service's
/// `TagKey` and `TagValue` carry (`smithy_query(query="shape", shape="TagKey,TagValue")`:
/// `@length(1, 128)` and `@length(0, 256)`), so the count row of that table is the quota
/// this service's resources are held to as well.
///
/// `TagList` is `@length(0, 200)`, which is only the widest list the boundary validator
/// lets through: a write that would leave a resource holding more distinct keys than this
/// quota is refused by the handler it came through ([`tags_past_cap_message`], a 400
/// `ValidationException` — the refusal this service is recorded answering a tag write past
/// the quota with), and stores nothing.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most specifiers one profile maps for one certificate field at once.
///
/// `MappingRules` carries no `@length`
/// (`smithy_query(query="shape", shape="MappingRules")`), so this ceiling is this
/// emulator's own bound rather than a published limit — the same construct
/// [`MAX_TRUST_ANCHORS`] is, and for the same reason: a profile's mappings live for the
/// process's lifetime and nothing evicts them. It sits far above the handful of specifiers
/// an X.509 field actually has, so a caller never meets it by accident, and a
/// `PutAttributeMapping` that would carry a field past it is refused with that operation's
/// modelled `ValidationException` rather than storing specifiers.
pub const MAX_MAPPING_RULES_PER_CERTIFICATE_FIELD: usize = 1_000;

/// The `sourceType` values a trust anchor can carry whose certificate data member the
/// model names: `x509CertificateData` is "Included for trust anchors of type
/// `CERTIFICATE_BUNDLE`" and `acmPcaArn` "Included for trust anchors of type
/// `AWS_ACM_PCA`" (`smithy_query(query="shape", shape="SourceData")`).
pub const CERTIFICATE_BUNDLE_SOURCE_TYPE: &str = "CERTIFICATE_BUNDLE";
/// See [`CERTIFICATE_BUNDLE_SOURCE_TYPE`].
pub const ACM_PCA_SOURCE_TYPE: &str = "AWS_ACM_PCA";

/// One stored trust anchor. Every `TrustAnchorDetail` member this service can know
/// about is persisted here; [`Self::to_detail`] is the only place a response is built
/// from it.
#[derive(Debug, Clone)]
pub struct TrustAnchorRecord {
    pub trust_anchor_id: String,
    pub trust_anchor_arn: String,
    pub name: String,
    pub source: Source,
    pub enabled: bool,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// Whether this trust anchor checks a presented certificate against the CRL
    /// distribution points the certificate itself names.
    ///
    /// Nothing in this service's surface turns it on: no modelled request member carries
    /// it, so a stored anchor keeps the value its create wrote. It lives on the record
    /// rather than in the projection so that an operation that learns to toggle it has
    /// one place to write, exactly like [`Self::enabled`].
    pub crl_dp_revocation_enabled: bool,
    /// This anchor's notification settings: one entry per key in force, covering every
    /// modelled `NotificationEvent` from the moment it is created. See
    /// [`default_notification_details`].
    pub notification_settings: Vec<NotificationSettingDetail>,
    pub tags: Vec<Tag>,
}

impl TrustAnchorRecord {
    /// The `TrustAnchorDetail` every trust-anchor operation returns.
    pub fn to_detail(&self) -> TrustAnchorDetail {
        TrustAnchorDetail {
            trust_anchor_id: Some(self.trust_anchor_id.clone()),
            trust_anchor_arn: Some(self.trust_anchor_arn.clone()),
            name: Some(self.name.clone()),
            source: Some(self.source.clone()),
            enabled: Some(self.enabled),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            notification_settings: Some(self.notification_settings.clone()),
            crl_dp_revocation_enabled: Some(self.crl_dp_revocation_enabled),
        }
    }
}

/// One stored certificate revocation list. Every `CrlDetail` member this service can
/// know about is persisted here; [`Self::to_detail`] is the only place a response is
/// built from it.
#[derive(Debug, Clone)]
pub struct CrlRecord {
    pub crl_id: String,
    pub crl_arn: String,
    pub name: String,
    pub enabled: bool,
    /// The PEM bytes the import (or a later update) carried, stored verbatim: `crlData`
    /// is "The state of the certificate revocation list (CRL) after a read or write
    /// operation" (`smithy_query(query="shape", shape="CrlDetail")`), so a read returns
    /// exactly the bytes that were written.
    pub crl_data: Vec<u8>,
    pub trust_anchor_arn: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// Tags the import carried. `CrlDetail` models no tag member, so they are reachable
    /// only through `ListTagsForResource`.
    pub tags: Vec<Tag>,
}

impl CrlRecord {
    /// The `CrlDetail` every CRL operation returns.
    pub fn to_detail(&self) -> CrlDetail {
        CrlDetail {
            crl_id: Some(self.crl_id.clone()),
            crl_arn: Some(self.crl_arn.clone()),
            name: Some(self.name.clone()),
            enabled: Some(self.enabled),
            crl_data: Some(self.crl_data.clone()),
            trust_anchor_arn: Some(self.trust_anchor_arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }
}

/// One stored profile. Every `ProfileDetail` member this service can know about is
/// persisted here; [`Self::to_detail`] is the only place a response is built from it.
#[derive(Debug, Clone)]
pub struct ProfileRecord {
    pub profile_id: String,
    pub profile_arn: String,
    pub name: String,
    pub role_arns: Vec<String>,
    pub managed_policy_arns: Vec<String>,
    pub session_policy: Option<String>,
    pub duration_seconds: i32,
    pub enabled: bool,
    pub require_instance_properties: bool,
    pub accept_role_session_name: bool,
    pub created_by: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// Certificate-field mappings applied to the authenticating end-entity certificate:
    /// at most one entry per `certificateField`, holding every specifier mapped for that
    /// field. A create starts it empty, and [`put_attribute_mapping`] /
    /// [`delete_attribute_mapping`] are the only writers.
    pub attribute_mappings: Vec<AttributeMapping>,
    pub tags: Vec<Tag>,
}

impl ProfileRecord {
    /// The `ProfileDetail` every profile operation returns.
    pub fn to_detail(&self) -> ProfileDetail {
        ProfileDetail {
            profile_id: Some(self.profile_id.clone()),
            profile_arn: Some(self.profile_arn.clone()),
            name: Some(self.name.clone()),
            require_instance_properties: Some(self.require_instance_properties),
            enabled: Some(self.enabled),
            created_by: Some(self.created_by.clone()),
            session_policy: self.session_policy.clone(),
            role_arns: Some(self.role_arns.clone()),
            managed_policy_arns: Some(self.managed_policy_arns.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            duration_seconds: Some(self.duration_seconds),
            accept_role_session_name: Some(self.accept_role_session_name),
            attribute_mappings: Some(self.attribute_mappings.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-rolesanywhere-rolesanywhere. A handler reaches it through [`service_state`].
///
/// Every map is a `BTreeMap` keyed by the resource's `Uuid`, so iteration order is a
/// stable total order and the `@paginated` list operations can resume from an opaque
/// cursor without keeping a server-side token table.
#[derive(Debug, Default)]
pub struct ComAmazonawsRolesanywhereRolesanywhereState {
    /// Every stored trust anchor, keyed by its `trustAnchorId`.
    pub trust_anchors: BTreeMap<String, TrustAnchorRecord>,
    /// Every stored profile, keyed by its `profileId`.
    pub profiles: BTreeMap<String, ProfileRecord>,
    /// Every stored certificate revocation list, keyed by its `crlId`.
    pub crls: BTreeMap<String, CrlRecord>,
    /// Every subject the native session plane has recorded a credential request for,
    /// keyed by its `subjectId`. See [`subjects`].
    pub subjects: BTreeMap<String, subjects::SubjectRecord>,
    /// Monotonic input to [`Self::mint_uuid`]; never returned to a caller.
    id_counter: u64,
}

impl ComAmazonawsRolesanywhereRolesanywhereState {
    /// A fresh resource id in the modelled `Uuid` form
    /// (`@length(36, 36)` + `@pattern("[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}")`),
    /// unused by any stored trust anchor, profile or certificate revocation list.
    ///
    /// Derived from a per-store counter and the wall clock — never from the request —
    /// so two identical requests still get distinct ids. Every character is lowercase
    /// hex, which satisfies both character classes in the pattern.
    pub fn mint_uuid(&mut self) -> String {
        loop {
            self.id_counter = self.id_counter.wrapping_add(1);
            let nanos = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0u128, |d| d.as_nanos());
            let mut hasher = Sha256::new();
            hasher.update(self.id_counter.to_le_bytes());
            hasher.update(nanos.to_le_bytes());
            let hex = hex::encode(&hasher.finalize()[..16]);
            let id = format!(
                "{}-{}-{}-{}-{}",
                &hex[0..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..32]
            );
            if !self.trust_anchors.contains_key(&id)
                && !self.profiles.contains_key(&id)
                && !self.crls.contains_key(&id)
                && !self.subjects.contains_key(&id)
            {
                return id;
            }
        }
    }

    /// The tags attached to the resource this ARN names, or `None` when no stored trust
    /// anchor, profile or certificate revocation list carries that ARN.
    ///
    /// The ARN is matched against the one every record was minted with, so an ARN is
    /// resolvable exactly as long as the resource it names exists.
    pub fn tags(&self, resource_arn: &str) -> Option<&Vec<Tag>> {
        self.trust_anchors
            .values()
            .find(|record| record.trust_anchor_arn == resource_arn)
            .map(|record| &record.tags)
            .or_else(|| {
                self.profiles
                    .values()
                    .find(|record| record.profile_arn == resource_arn)
                    .map(|record| &record.tags)
            })
            .or_else(|| {
                self.crls
                    .values()
                    .find(|record| record.crl_arn == resource_arn)
                    .map(|record| &record.tags)
            })
    }

    /// The tags attached to the resource this ARN names, for writing. See [`Self::tags`].
    pub fn tags_mut(&mut self, resource_arn: &str) -> Option<&mut Vec<Tag>> {
        if let Some(record) = self
            .trust_anchors
            .values_mut()
            .find(|record| record.trust_anchor_arn == resource_arn)
        {
            return Some(&mut record.tags);
        }
        if let Some(record) = self
            .profiles
            .values_mut()
            .find(|record| record.profile_arn == resource_arn)
        {
            return Some(&mut record.tags);
        }
        self.crls
            .values_mut()
            .find(|record| record.crl_arn == resource_arn)
            .map(|record| &mut record.tags)
    }
}

/// Merge the tags a request carried into the tags a resource already holds, keyed by tag
/// key: a key the resource already carries takes the submitted value, a key it does not
/// carry is added, and a key the request does not name keeps the value it had. So one
/// tagging call never drops another's tags, and re-tagging a key replaces its value in
/// place rather than attaching the key twice.
pub fn merge_tags(held: &mut Vec<Tag>, incoming: Vec<Tag>) {
    for tag in incoming {
        match held.iter_mut().find(|existing| existing.key == tag.key) {
            Some(existing) => existing.value = tag.value,
            None => held.push(tag),
        }
    }
}

/// Detach the keys a request named from the tags a resource holds, keyed by tag key: a
/// key the resource carries is dropped with its value, a key it does not carry leaves the
/// set alone, and a key the request does not name is kept. So the set afterwards is
/// exactly what the resource held minus the named keys, whatever the order the keys come
/// in and however often one is repeated — which makes removing a key idempotent: a
/// repeated call reaches the same set rather than failing.
pub fn remove_tags(held: &mut Vec<Tag>, keys: &[String]) {
    held.retain(|tag| !keys.iter().any(|key| key == &tag.key));
}

/// How many tags the resource would hold after [`merge_tags`]: the number of distinct
/// keys across what it holds and what the request names, because a merge replaces a
/// value in place rather than adding a second entry for the same key.
pub fn merged_tag_count(held: &[Tag], incoming: &[Tag]) -> usize {
    held.iter()
        .chain(incoming)
        .map(|tag| tag.key.as_str())
        .collect::<BTreeSet<&str>>()
        .len()
}

/// The tag set a resource starts life with, from the list its create carried: the same
/// key-addressed set [`merge_tags`] maintains, so a key the request names twice is held
/// once, with the value that came last, exactly as re-tagging it afterwards would leave
/// it. A tag set is addressed by key wherever tags enter a resource.
pub fn keyed_tags(submitted: Vec<Tag>) -> Vec<Tag> {
    let mut held = Vec::new();
    merge_tags(&mut held, submitted);
    held
}

/// The `message` a write that would leave a resource holding more than
/// [`MAX_TAGS_PER_RESOURCE`] tags is refused with: the fixed wording this service answers
/// a tag write past the quota with, naming the quota as its only variable part. It
/// describes the RESULTANT set — what the resource would hold afterwards — which is what
/// the quota bounds. No tag is echoed into it, since `TagKey` and `TagValue` are both
/// `@sensitive`.
pub fn tags_past_cap_message() -> String {
    format!("The resultant tag set must not have more than {MAX_TAGS_PER_RESOURCE} user tags.")
}

/// One page of a `BTreeMap`-backed collection, plus the cursor to resume from.
///
/// `after` is start-EXCLUSIVE: the page begins just past the key the previous page
/// ended on, so across pages every resource is enumerated exactly once — no duplicates
/// and no skips, even when a resource is created or deleted between two pages. A
/// `nextToken` comes back only when entries remain beyond this page.
/// The work is O(`page_size`), not O(store): the map is walked through
/// [`BTreeMap::range`] from the cursor and at most ONE entry past the page is ever
/// looked at — that entry's existence is what says another page remains.
pub fn page<'a, T>(
    items: &'a BTreeMap<String, T>,
    after: Option<&str>,
    page_size: usize,
) -> (Vec<&'a T>, Option<String>) {
    let lower = match after {
        Some(cursor) => ::std::ops::Bound::Excluded(cursor.to_string()),
        None => ::std::ops::Bound::Unbounded,
    };
    let window = items
        .range((lower, ::std::ops::Bound::Unbounded))
        .take(page_size.saturating_add(1));

    let mut entries: Vec<&'a T> = Vec::new();
    let mut last_key: Option<&String> = None;
    let mut more = false;
    for (index, (key, value)) in window.enumerate() {
        if index == page_size {
            more = true;
            break;
        }
        last_key = Some(key);
        entries.push(value);
    }

    let next_token = last_key
        .filter(|_| more)
        .map(|key| encode_next_token(key));
    (entries, next_token)
}

/// The page size a list request asks for: the modelled `pageSize`, or [`MAX_PAGE_SIZE`]
/// when it is absent, bounded above by [`MAX_PAGE_SIZE`] so one response is always a
/// bounded page whatever reaches the handler.
///
/// `None` when the request named a non-positive page, which is no page at all: the
/// caller is told so rather than being handed a one-entry page it did not ask for.
pub fn page_size(requested: Option<i32>) -> Option<usize> {
    let requested = requested.unwrap_or(MAX_PAGE_SIZE);
    if requested < 1 {
        return None;
    }
    usize::try_from(requested.min(MAX_PAGE_SIZE)).ok()
}

/// The wording a list operation refuses a non-positive `pageSize` with.
pub fn non_positive_page_size_message() -> String {
    validation_message(
        "pageSize",
        "Member must have value greater than or equal to 1",
    )
}

/// Encode a list cursor as an opaque `nextToken`.
///
/// Hex of the last key this page returned: it satisfies the modelled `nextToken`
/// `@length(1, 10000)`, is opaque to the caller, and carries the whole cursor — so
/// paging needs no server-side token table, which matters because a `@readonly` list
/// operation must not write to state.
pub fn encode_next_token(key: &str) -> String {
    hex::encode(key)
}

/// The key a `nextToken` points just past, or `None` when the token is not one this
/// service issued.
pub fn decode_next_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// The `message` a `ValidationException` carries for one member that failed one
/// constraint, worded the way the boundary validator words its own refusals. Shared so
/// every operation words a refusal identically.
pub fn validation_message(member: &str, constraint: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: {constraint}"
    )
}

/// The `message` a `ResourceNotFoundException` carries when the resource a request names
/// by ARN is not stored: the fixed wording this service words an ARN-keyed miss with,
/// whose only variable part is the ARN the request carried.
pub fn arn_not_found_message(resource_arn: &str) -> String {
    format!("The item doesn't exist. arn: {resource_arn} ; aborting.")
}

/// The ARN of the certificate revocation list this id names, in the Region the request
/// was signed for: the ARN its import minted, so a refusal naming a CRL that is not
/// stored words the SAME ARN the read of a stored one answers with.
pub fn crl_arn(region: &str, crl_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("crl/{crl_id}"))
}

/// The ARN of the profile this id names, in the Region the request was signed for: the
/// ARN its create minted, so a refusal naming a profile that is not stored words the
/// SAME ARN the read of a stored one answers with.
///
/// `ProfileArn` is `@pattern("^arn:aws(-[^:]+)?:rolesanywhere(:.*){2}(:profile.*)$")`
/// (`smithy_query(query="shape", shape="ProfileDetail")`, member `profileArn`), which this
/// satisfies: `arn:aws:rolesanywhere:<region>:<account>:profile/<profileId>`.
pub fn profile_arn(region: &str, profile_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("profile/{profile_id}"))
}

/// The trust-anchor id a `TrustAnchorArn` names, or the empty string when the ARN carries
/// no id.
///
/// `TrustAnchorArn` is `@pattern("^arn:aws(-[^:]+)?:rolesanywhere(:.*){2}(:trust-anchor.*)$")`
/// (`smithy_query(query="operation", operation="ImportCrl")`), so the resource — everything
/// past the fifth colon of `arn:<partition>:rolesanywhere:<region>:<account>:<resource>` —
/// starts with `trust-anchor` and names the id after a separator. Parsing is total: a
/// resource part that is not `trust-anchor` plus a separator plus an id yields the empty
/// string, which no trust anchor can be stored under.
pub fn trust_anchor_id_in_arn(trust_anchor_arn: &str) -> &str {
    let resource = trust_anchor_arn.splitn(6, ':').nth(5).unwrap_or_default();
    resource
        .strip_prefix("trust-anchor/")
        .or_else(|| resource.strip_prefix("trust-anchor:"))
        .unwrap_or_default()
}

/// The Region an ARN of this service names: the fourth field of
/// `arn:<partition>:rolesanywhere:<region>:<account>:<resource>`, or `None` when the value
/// carries no fourth field or that field is empty.
///
/// Read off an ARN this service MINTED, which is how a request is attributed to a Region
/// without taking the Region from the request: the resource the request resolved against
/// already carries the one its create was signed for.
pub fn region_in_arn(arn: &str) -> Option<&str> {
    arn.split(':').nth(3).filter(|region| !region.is_empty())
}

/// The `message` a refusal carries when a request REFERENCES, by ARN, a trust anchor
/// that is not stored: the fixed wording, whose only variable part is the id taken from
/// the ARN the request carried.
pub fn trust_anchor_not_found_message(trust_anchor_id: &str) -> String {
    format!("Unable to find Trust Anchor with ID {trust_anchor_id}.")
}

/// The `message` a refusal carries when the trust anchor a request ADDRESSES by
/// `trustAnchorId` is not stored.
///
/// Wholly fixed text, with no variable part: the id is already the resource the request
/// named, and this is the wording the service is recorded answering such a miss with —
/// unlike the ARN-reference wording above, which belongs to an operation that names a
/// trust anchor while addressing something else.
pub fn addressed_trust_anchor_not_found_message() -> String {
    "TrustAnchor not found".to_string()
}

/// The region this request was signed for, from its SigV4 credential scope; an
/// unsigned request is attributed to [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The principal `NotificationSettingDetail$configuredBy` names for a setting this
/// service configured: "For default settings configured by IAM Roles Anywhere, the value
/// is `rolesanywhere.amazonaws.com`, and for customized notifications settings, it is the
/// respective account ID"
/// (`smithy_query(query="shape", shape="NotificationSettingDetail")`, member
/// `configuredBy`).
pub const SERVICE_NOTIFICATION_PRINCIPAL: &str = SERVICE_PRINCIPAL;

/// The principal this service speaks for when it acts on an account's behalf: the service
/// principal a role's trust policy names to let IAM Roles Anywhere assume it, which is
/// also the spelling above.
pub const SERVICE_PRINCIPAL: &str = "rolesanywhere.amazonaws.com";

/// The `threshold` — "the number of days before a notification event" — a notification
/// setting carries while this service, rather than the caller, configures it.
///
/// The model publishes no default for the member (it carries `@range(1, 360)` and nothing
/// else), so this is the value the service is RECORDED to seed a trust anchor's settings
/// with, not a modelled constant.
pub const DEFAULT_NOTIFICATION_THRESHOLD_DAYS: i32 = 45;

/// The channel a notification setting that names none applies to: "In the absence of a
/// specific channel, IAM Roles Anywhere applies this setting to 'ALL' channels"
/// (`smithy_query(query="doc", shape="NotificationSetting", member="channel")`). It is
/// also the only value the modelled `NotificationChannel` enum accepts.
pub const DEFAULT_NOTIFICATION_CHANNEL: &str = "ALL";

/// The notification setting this service configures for one event until a caller
/// customizes it: enabled, at [`DEFAULT_NOTIFICATION_THRESHOLD_DAYS`], on every channel,
/// and configured by [`SERVICE_NOTIFICATION_PRINCIPAL`].
fn default_notification_detail(event: &str, channel: &str) -> NotificationSettingDetail {
    NotificationSettingDetail {
        enabled: true,
        event: event.to_string(),
        threshold: Some(DEFAULT_NOTIFICATION_THRESHOLD_DAYS),
        channel: Some(channel.to_string()),
        configured_by: Some(SERVICE_NOTIFICATION_PRINCIPAL.to_string()),
    }
}

/// The notification settings a trust anchor holds before any caller customizes one: the
/// service default for EVERY value of the modelled `NotificationEvent` enum
/// (`NOTIFICATION_EVENT_VALUES`), in the order the model lists them.
///
/// A trust anchor is never without its expiry notifications: nothing in the surface
/// deletes a setting, so this set is the floor a read reports and the state a reset
/// returns one key to.
pub fn default_notification_details() -> Vec<NotificationSettingDetail> {
    NOTIFICATION_EVENT_VALUES
        .iter()
        .map(|event| default_notification_detail(event, DEFAULT_NOTIFICATION_CHANNEL))
        .collect()
}

/// The key one notification setting is held under: its event and its channel, the pair
/// `NotificationSettingKey` is made of ("A notification setting key includes the event and
/// the channel"), with an unnamed channel resolved to
/// [`DEFAULT_NOTIFICATION_CHANNEL`].
fn notification_key(event: &str, channel: Option<&str>) -> (String, String) {
    (
        event.to_string(),
        channel.unwrap_or(DEFAULT_NOTIFICATION_CHANNEL).to_string(),
    )
}

/// Store one setting under its key: it REPLACES the setting the anchor held for that
/// event and channel, and is appended only when the anchor holds none — so a write
/// changes one key and leaves every other key as it was.
fn put_notification_detail(
    held: &mut Vec<NotificationSettingDetail>,
    detail: NotificationSettingDetail,
) {
    let key = notification_key(&detail.event, detail.channel.as_deref());
    match held
        .iter_mut()
        .find(|existing| notification_key(&existing.event, existing.channel.as_deref()) == key)
    {
        Some(existing) => *existing = detail,
        None => held.push(detail),
    }
}

/// Apply the `NotificationSetting`s a request carried to the settings a trust anchor
/// holds: each one is stored as submitted — enabled, threshold and channel as sent, with
/// an unnamed channel resolved to [`DEFAULT_NOTIFICATION_CHANNEL`] — under its own key,
/// and stamped `configuredBy` with the caller's account, because "for customized
/// notifications settings, it is the respective account ID"
/// (`NotificationSettingDetail$configuredBy` doc).
pub fn apply_notification_settings(
    held: &mut Vec<NotificationSettingDetail>,
    submitted: Vec<NotificationSetting>,
) {
    for setting in submitted {
        let detail = NotificationSettingDetail {
            enabled: setting.enabled,
            event: setting.event,
            threshold: setting.threshold,
            channel: Some(
                setting
                    .channel
                    .unwrap_or_else(|| DEFAULT_NOTIFICATION_CHANNEL.to_string()),
            ),
            configured_by: Some(::engine::emulator_account_id().to_string()),
        };
        put_notification_detail(held, detail);
    }
}

/// Return the keys a request named to the setting this service configures for them —
/// "Resets the custom notification setting to IAM Roles Anywhere default setting"
/// (`ResetNotificationSettings` doc) — leaving every key the request did not name as the
/// caller customized it.
pub fn reset_notification_settings(
    held: &mut Vec<NotificationSettingDetail>,
    keys: &[NotificationSettingKey],
) {
    for key in keys {
        let channel = key
            .channel
            .as_deref()
            .unwrap_or(DEFAULT_NOTIFICATION_CHANNEL);
        put_notification_detail(held, default_notification_detail(&key.event, channel));
    }
}

/// Whether one `AttributeMapping` is the one held for `certificate_field`.
///
/// At most ONE entry per certificate field, because a mapping "specifies a certificate
/// field and one or more specifiers"
/// (`smithy_query(query="doc", shape="PutAttributeMapping")`): the field is the key and its
/// `mappingRules` are the value, so two entries for one field could not both be enforced.
fn maps_field(mapping: &AttributeMapping, certificate_field: &str) -> bool {
    mapping.certificate_field.as_deref() == Some(certificate_field)
}

/// Put the submitted rules into the rules a profile enforces for one certificate field:
/// "Put an entry in the attribute mapping rules that will be enforced by a given profile"
/// (`PutAttributeMapping` doc), where an entry is "a single mapping entry for each
/// supported specifier or sub-field" (`MappingRule` doc).
///
/// So each submitted specifier joins the specifiers the field already maps; a specifier the
/// field already maps keeps its single entry, which is what makes the write `@idempotent`
/// as the model declares it; and the rules of every other certificate field are untouched.
/// A field is given an entry only once it maps at least one specifier.
pub fn put_attribute_mapping(
    held: &mut Vec<AttributeMapping>,
    certificate_field: &str,
    submitted: Vec<MappingRule>,
) {
    if submitted.is_empty() {
        return;
    }
    if !held
        .iter()
        .any(|mapping| maps_field(mapping, certificate_field))
    {
        held.push(AttributeMapping {
            certificate_field: Some(certificate_field.to_string()),
            mapping_rules: Some(Vec::new()),
        });
    }
    let Some(mapping) = held
        .iter_mut()
        .find(|mapping| maps_field(mapping, certificate_field))
    else {
        return;
    };
    let rules = mapping.mapping_rules.get_or_insert_with(Vec::new);
    for rule in submitted {
        if !rules.iter().any(|mapped| mapped.specifier == rule.specifier) {
            rules.push(rule);
        }
    }
}

/// How many specifiers the field would map after [`put_attribute_mapping`]: the number of
/// DISTINCT specifiers across what the field already maps and what the request carries,
/// because the put adds a specifier the field already maps no second time.
pub fn merged_mapping_rule_count(
    held: &[AttributeMapping],
    certificate_field: &str,
    submitted: &[MappingRule],
) -> usize {
    let mapped = held
        .iter()
        .find(|mapping| maps_field(mapping, certificate_field))
        .and_then(|mapping| mapping.mapping_rules.as_deref())
        .unwrap_or_default();
    mapped
        .iter()
        .chain(submitted)
        .map(|rule| rule.specifier.as_str())
        .collect::<BTreeSet<&str>>()
        .len()
}

/// Delete rules from the rules a profile enforces for one certificate field: "Delete an
/// entry from the attribute mapping rules enforced by a given profile"
/// (`DeleteAttributeMapping` doc).
///
/// `specifiers` is optional where `certificateField` is `@required`
/// (`smithy_query(query="operation", operation="DeleteAttributeMapping")`), so a request
/// that names specifiers deletes exactly those from the field, and one that names none
/// deletes everything the field maps. A specifier the field does not map is already absent,
/// so deleting it changes nothing — the write is `@idempotent`, as the model declares. A
/// field left mapping no specifier keeps no entry, because a mapping specifies a field AND
/// one or more specifiers.
pub fn delete_attribute_mapping(
    held: &mut Vec<AttributeMapping>,
    certificate_field: &str,
    specifiers: Option<&[String]>,
) {
    let Some(specifiers) = specifiers.filter(|specifiers| !specifiers.is_empty()) else {
        held.retain(|mapping| !maps_field(mapping, certificate_field));
        return;
    };
    if let Some(mapping) = held
        .iter_mut()
        .find(|mapping| maps_field(mapping, certificate_field))
    {
        if let Some(rules) = mapping.mapping_rules.as_mut() {
            rules.retain(|rule| !specifiers.contains(&rule.specifier));
        }
    }
    // A field left mapping no specifier is no longer a mapping at all.
    held.retain(|mapping| {
        !maps_field(mapping, certificate_field)
            || mapping
                .mapping_rules
                .as_ref()
                .is_some_and(|rules| !rules.is_empty())
    });
}

/// The member and the constraint a `Source` fails, or `None` when it describes a
/// certificate authority this service can store.
///
/// `Source`'s own two members are optional even where the `Source` itself is
/// `@required`, and the model names which certificate data member belongs to which
/// `sourceType`, so a source that carries neither — or carries the data of the other
/// type — describes no certificate authority at all. Shared, because every operation that
/// accepts a `Source` has to judge it the same way.
pub fn source_rejection(source: &Source) -> Option<(&'static str, String)> {
    let source_type = match source
        .source_type
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        Some(source_type) => source_type,
        None => return Some(("source.sourceType", "Member must not be null".to_string())),
    };
    if !TRUST_ANCHOR_TYPE_VALUES.contains(&source_type) {
        return Some((
            "source.sourceType",
            format!(
                "Member must satisfy enum value set: [{}]",
                TRUST_ANCHOR_TYPE_VALUES.join(", ")
            ),
        ));
    }
    let Some(source_data) = source.source_data.as_ref() else {
        return Some(("source.sourceData", "Member must not be null".to_string()));
    };
    let expected_data_member = match source_type {
        CERTIFICATE_BUNDLE_SOURCE_TYPE => Some("x509CertificateData"),
        ACM_PCA_SOURCE_TYPE => Some("acmPcaArn"),
        // `SELF_SIGNED_REPOSITORY` is the third modelled `sourceType` and the model ties
        // neither `SourceData` member to it, so no pairing can be asserted for it and
        // whatever the caller sent is stored as sent.
        _ => None,
    };
    let actual_data_member = match source_data {
        SourceData::x509CertificateData(_) => "x509CertificateData",
        SourceData::acmPcaArn(_) => "acmPcaArn",
    };
    expected_data_member
        .filter(|member| *member != actual_data_member)
        .map(|expected| {
            (
                "source.sourceData",
                format!("Member must be {expected} for a trust anchor of type {source_type}"),
            )
        })
}

/// The failure a `roleArns` list carries when one of its entries is not an IAM role ARN,
/// as the `(member, constraint)` pair [`validation_message`] words a refusal from — or
/// `None` when every entry is one.
///
/// A profile vends credentials for the roles it names, so every entry has to be an IAM
/// role ARN. Shared, because every operation that accepts a `roleArns` list has to judge
/// it the same way: a list one of them stores is a list the others accept.
pub fn role_arns_rejection(role_arns: &[String]) -> Option<(String, &'static str)> {
    role_arns
        .iter()
        .position(|role_arn| !is_iam_role_arn(role_arn))
        .map(|index| {
            (
                // Elements are numbered from one, the way the boundary validator numbers
                // the element of a list its own constraint check refuses, so one list is
                // addressed identically whichever check catches it.
                format!("roleArns.{}.member", index + 1),
                "Member must be an IAM role ARN of the form \
                 arn:<partition>:iam::<account>:role/<name>",
            )
        })
}

/// Whether `candidate` is an IAM role ARN: `arn:<partition>:iam::<account>:role/<path>`.
///
/// IAM is a global service, so the region segment of an IAM ARN is empty — which is
/// why the segments are checked rather than just the prefix.
fn is_iam_role_arn(candidate: &str) -> bool {
    let mut segments = candidate.splitn(6, ':');
    let (Some("arn"), Some(partition), Some("iam"), Some(""), Some(account), Some(resource)) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return false;
    };
    !partition.is_empty()
        && account.chars().all(|c| c.is_ascii_digit())
        && !account.is_empty()
        && resource.starts_with("role/")
        && resource.len() > "role/".len()
}

/// The failure a handler answers when this service's state cannot be reached — i.e.
/// the emulator was built on an engine type this service cannot use, so the operation
/// cannot be served at all. Unreachable in the assembled emulator, which builds exactly
/// one `InMemoryEngine`; never answer it by constructing a private engine, whose writes
/// no later request can see.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "this emulator was built on an engine this service cannot reach its state \
                  through, so no IAM Roles Anywhere operation can be served"
            .to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRolesanywhereRolesanywhereState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRolesanywhereRolesanywhereState>)
}
