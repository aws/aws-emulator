// SPDX-License-Identifier: CC0-1.0

//! The OpsItems OpsCenter holds, and the rules a create of one is held to.
//!
//! An OpsItem is "an operational work item" OpsCenter tracks. `CreateOpsItem` mints one,
//! `UpdateOpsItem` edits it and `GetOpsItem` reports it, all through the one store below
//! ([`OpsItemRecord`]), so whatever the last write persisted is what a later read reports:
//! nothing in a response is invented at read time.
//!
//! An OpsItem is identified by a server-minted id of the modelled `OpsItemId` form
//! (`^(oi)-[0-9a-f]{12}$`, [`mint_ops_item_id`]) and is reached through the endpoint of
//! the Region it was created in, so the store is keyed by `(region, ops item id)`.
//!
//! Three rules a create is held to are documented rather than modelled, so they are
//! stated here once and applied by every operation that writes an OpsItem:
//!
//! * a `Source` "can't contain the following strings: aws, amazon, and amzn"
//!   ([`SOURCE_FORBIDDEN_SUBSTRINGS`], [`source_rejection`]);
//! * an operational-data key "can't begin with the following: amazon, aws, amzn, ssm,
//!   /amazon, /aws, /amzn, /ssm" ([`RESERVED_OPERATIONAL_DATA_PREFIXES`]) except for the
//!   keys the service itself defines under those prefixes
//!   ([`SERVICE_OPERATIONAL_DATA_KEYS`]), and an operational-data value "has a maximum
//!   size of 20 KB" ([`MAX_OPERATIONAL_DATA_VALUE`]) — [`operational_data_rejection`];
//! * the `/aws/dedup` key makes a create idempotent against the OpsItems already open
//!   with the same dedup string ([`dedup_string`]), which is the one thing
//!   `OpsItemAlreadyExistsException` reports.

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::{ComAmazonawsSsmAmazonssmState, ARN_SERVICE};
use crate::ops::types::{
    OpsItem, OpsItemDataValue, OpsItemNotification, OpsItemSummary, RelatedOpsItem,
};

/// The resource portion an OpsItem ARN carries before the OpsItem's id, e.g.
/// `arn:aws:ssm:us-east-1:111122223333:opsitem/oi-0dd0a6b47d6b`, which is what the
/// modelled `OpsItemArn` pattern (`…:[0-9]{12}:opsitem`) begins with.
pub const ARN_OPS_ITEM_RESOURCE_PREFIX: &str = "opsitem/";

/// What every OpsItem id starts with, from the modelled `OpsItemId` pattern
/// `^(oi)-[0-9a-f]{12}$`.
pub const OPS_ITEM_ID_PREFIX: &str = "oi-";

/// How many hex digits an OpsItem id carries after [`OPS_ITEM_ID_PREFIX`], from the same
/// modelled pattern.
pub const OPS_ITEM_ID_HEX_DIGITS: usize = 12;

/// The OpsItem types "Systems Manager supports" (`CreateOpsItem`'s `OpsItemType` doc:
/// "/aws/issue", "/aws/insight", "/aws/changerequest").
pub const OPS_ITEM_TYPES: &[&str] = &["/aws/issue", "/aws/insight", "/aws/changerequest"];

/// The type of "default OpsItems created by OpsCenter" (the same doc), and so the type of
/// an OpsItem whose create named none.
pub const DEFAULT_OPS_ITEM_TYPE: &str = "/aws/issue";

/// The `OpsItemStatus` value a created OpsItem is in: an operational work item nobody has
/// worked yet is open. It is one of the modelled enum's values
/// (`crate::ops::types::OPS_ITEM_STATUS_VALUES`).
pub const OPS_ITEM_STATUS_OPEN: &str = "Open";

/// The `OpsItemStatus` values an OpsItem is no longer being worked in, and so no longer
/// deduplicates against ([`ComAmazonawsSsmAmazonssmState::open_ops_item_with_dedup`]).
/// Both are values of the modelled enum.
pub const OPS_ITEM_TERMINAL_STATUSES: &[&str] = &["Resolved", "Closed"];

/// How long a delete of an OpsItem takes to settle, after which the record is gone.
///
/// `DeleteOpsItem` "uses an eventual consistency model, which means the system can take a
/// few minutes to complete this operation. If you delete an OpsItem and immediately call,
/// for example, GetOpsItem, the deleted OpsItem might still appear in the response", and
/// an OpsItem being deleted is the state `OpsItemConflictException` ("The specified OpsItem
/// is in the process of being deleted") reports. That doc bounds the wait at "a few
/// minutes" without naming one, so this is the emulator's CHOSEN window inside it: ten
/// seconds, long enough that a caller reading an OpsItem straight after deleting it is
/// served what the real service serves it, and short enough that a caller waiting for the
/// delete to confirm does not wait minutes for it. The removal is materialised from the
/// stored due instant
/// ([`ComAmazonawsSsmAmazonssmState::settle_ops_item_deletions_due`]) rather than by any
/// call, and nothing sleeps for it.
pub const OPS_ITEM_DELETE_SETTLES_AFTER: ::std::time::Duration =
    ::std::time::Duration::from_secs(10);

/// The `Version` token a write of an OpsItem leaves on the record.
///
/// `OpsItem$Version` is "the version of this OpsItem. Each time the OpsItem is edited the
/// version number increments by one", and what Systems Manager mints for it is a token of
/// the instant of the write — the milliseconds of `at` since the epoch — so a later write
/// always carries a greater token than the write before it and a client can tell one
/// version of an OpsItem from the next by comparing them.
///
/// `previous` is the token the record already carries, when a write is editing one: two
/// writes of the same millisecond would otherwise leave the one token, so the token is
/// advanced past the previous one instead. The result is therefore STRICTLY increasing per
/// write of one OpsItem, whatever the clock's resolution, which is what a client comparing
/// versions to detect its own edit relies on.
pub fn minted_ops_item_version(at: SystemTime, previous: Option<u64>) -> u64 {
    let since_epoch = at
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let minted = u64::try_from(since_epoch).unwrap_or(u64::MAX);
    match previous {
        Some(previous) if previous >= minted => previous.saturating_add(1),
        _ => minted,
    }
}

/// The strings a `Source` may not contain: "The source name can't contain the following
/// strings: aws, amazon, and amzn" (`CreateOpsItem`'s `Source` doc).
pub const SOURCE_FORBIDDEN_SUBSTRINGS: &[&str] = &["aws", "amazon", "amzn"];

/// What an operational-data key may not begin with: "Operational data keys can't begin
/// with the following: amazon, aws, amzn, ssm, /amazon, /aws, /amzn, /ssm"
/// (`CreateOpsItem`'s `OperationalData` doc).
pub const RESERVED_OPERATIONAL_DATA_PREFIXES: &[&str] = &[
    "amazon", "aws", "amzn", "ssm", "/amazon", "/aws", "/amzn", "/ssm",
];

/// The operational-data keys Systems Manager itself defines under those reserved
/// prefixes, and so the ones a request may send: the `OperationalData` doc names
/// `/aws/resources` ("to specify a related resource in the request") and
/// `/aws/automations` ("to associate an Automation runbook with the OpsItem"), and
/// `/aws/dedup` is the key that carries the dedup string this create deduplicates on
/// ([`DEDUP_OPERATIONAL_DATA_KEY`]).
pub const SERVICE_OPERATIONAL_DATA_KEYS: &[&str] = &[
    RELATED_RESOURCES_OPERATIONAL_DATA_KEY,
    RELATED_AUTOMATIONS_OPERATIONAL_DATA_KEY,
    DEDUP_OPERATIONAL_DATA_KEY,
];

/// The operational-data key whose value "specif[ies] a related resource in the request"
/// (`CreateOpsItem`'s `OperationalData` doc), and so the one the `ResourceId` filter key of a
/// query reads ([`super::ops_item_queries`]).
pub const RELATED_RESOURCES_OPERATIONAL_DATA_KEY: &str = "/aws/resources";

/// The operational-data key whose value "associate[s] an Automation runbook with the OpsItem"
/// (the same doc), and so the one the `AutomationId` filter key of a query reads.
pub const RELATED_AUTOMATIONS_OPERATIONAL_DATA_KEY: &str = "/aws/automations";

/// The operational-data key whose value carries a dedup string: an OpsItem created with
/// one is the same work item as an OpsItem already open with the same string, which is
/// what `OpsItemAlreadyExistsException` ("The OpsItem already exists") reports.
pub const DEDUP_OPERATIONAL_DATA_KEY: &str = "/aws/dedup";

/// The member of the `/aws/dedup` value that carries the dedup string.
pub const DEDUP_STRING_MEMBER: &str = "dedupString";

/// "The value has a maximum size of 20 KB" (`CreateOpsItem`'s `OperationalData` doc). The
/// key's own 1..128 length is MODELLED, so the boundary walk already refuses it.
pub const MAX_OPERATIONAL_DATA_VALUE: usize = 20 * 1024;

/// How many operational-data keys ONE OpsItem holds at most.
///
/// Emulator resource guard, NOT a published quota: nothing in the model bounds the
/// `OpsItemOperationalData` map, and an OpsItem can be edited without end with every edit
/// free to add keys to it, so without this bound one OpsItem's map would grow without end
/// over a caller-chosen key space. It is enforced by the ONE rule both writes of an OpsItem
/// hold their map against ([`operational_data_rejection`]), so it holds however a caller
/// reached the store, and the refusal says the bound is the emulator's own.
pub const MAX_OPERATIONAL_DATA_KEYS: usize = 100;

/// The `OpsItemDataType` values the model's enum declares, and so the types an
/// operational-data value may name.
pub const OPS_ITEM_DATA_TYPE_SEARCHABLE_STRING: &str = "SearchableString";

/// The severities an OpsItem may carry: "Severity options range from 1 to 4"
/// (`OpsItem$Severity`'s doc).
pub const OPS_ITEM_SEVERITIES: &[&str] = &["1", "2", "3", "4"];

/// The categories an OpsItem may carry: "Category options include: Availability, Cost,
/// Performance, Recovery, Security" (`OpsItem$Category`'s doc).
pub const OPS_ITEM_CATEGORIES: &[&str] = &[
    "Availability",
    "Cost",
    "Performance",
    "Recovery",
    "Security",
];

/// The most OpsItems this emulator holds across every Region.
///
/// The Region an OpsItem is stored under comes from the request's own credential scope,
/// which the caller chooses, so without a total bound the store would grow without end
/// over a caller-chosen partition of the key space. The published OpsCenter quota is NOT
/// restated here: this bound is the emulator's own, and the refusal says so. It is
/// enforced at the store's ONE write path
/// ([`ComAmazonawsSsmAmazonssmState::insert_ops_item`]), so no caller can add a record
/// past it however it reached the store.
pub const MAX_OPS_ITEMS: usize = 10_000;

/// The fixed sentence a client reads for an id the Region holds no OpsItem under, stated
/// once here so every operation that cannot resolve an OpsItem id serves one wording.
///
/// This is the sentence Systems Manager words the class with, not
/// `OpsItemNotFoundException`'s Smithy documentation: a refusal is read by clients and SDKs
/// as the service words it, and the service words this one as the whole of
/// [`ops_item_not_found_message`].
pub const OPS_ITEM_NOT_FOUND_BODY: &str = "OpsItem not found.";

/// The error code that refusal is reported under, which is the modelled exception's own
/// shape name (`com.amazonaws.ssm#OpsItemNotFoundException`), and so what the envelope
/// below names as its code.
pub const OPS_ITEM_NOT_FOUND_CODE: &str = "OpsItemNotFoundException";

/// The HTTP status that refusal carries, which is the status the model declares for it
/// (`OpsItemNotFoundException`, httpError 400), and so what the envelope below names as its
/// status.
pub const OPS_ITEM_NOT_FOUND_STATUS: u16 = 400;

/// How this service reports an OpsItem id the Region holds no OpsItem under:
/// [`OPS_ITEM_NOT_FOUND_BODY`] followed by the envelope this service appends to a refusal it
/// words itself ([`super::with_sdk_error_envelope`]), whose only variable part is the serving
/// response's own request id.
///
/// A function rather than a constant because that request id is minted per call.
pub fn ops_item_not_found_message() -> String {
    super::with_sdk_error_envelope(
        OPS_ITEM_NOT_FOUND_BODY,
        OPS_ITEM_NOT_FOUND_STATUS,
        OPS_ITEM_NOT_FOUND_CODE,
    )
}

/// Which member of a create was refused, and why — the payload of
/// `OpsItemInvalidParameterException`, whose `ParameterNames` names the member and whose
/// `Message` says what the rule is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterRejection {
    /// The input member the rule belongs to, for `ParameterNames`.
    pub parameter: String,
    /// Why the value does not satisfy it.
    pub message: String,
}

impl ParameterRejection {
    /// A rejection of `parameter` because of `message`.
    pub(crate) fn new(parameter: &str, message: impl Into<String>) -> Self {
        Self {
            parameter: parameter.to_string(),
            message: message.into(),
        }
    }
}

/// Why `source` is not a usable `Source`, or `None` when it is.
///
/// The rule is the documented one: a source name may not CONTAIN `aws`, `amazon` or
/// `amzn`. The comparison is case-insensitive, because the rule is about the name
/// carrying the service's own marks rather than about one spelling of them.
pub fn source_rejection(source: &str) -> Option<ParameterRejection> {
    let folded = source.to_ascii_lowercase();
    SOURCE_FORBIDDEN_SUBSTRINGS
        .iter()
        .any(|forbidden| folded.contains(forbidden))
        .then(|| {
            ParameterRejection::new(
                "Source",
                format!(
                    "The source name can't contain the following strings: {}",
                    SOURCE_FORBIDDEN_SUBSTRINGS.join(", ")
                ),
            )
        })
}

/// Why `operational_data` is not usable, or `None` when it is.
///
/// Two documented rules, in the order a caller hits them: a key may not begin with one of
/// the reserved prefixes unless it is a key the service itself defines, and a value is at
/// most 20 KB. The key's own length and the value's own pattern are modelled, so the
/// boundary walk has already refused anything outside them. Beside them is the emulator's
/// own bound on how many keys one map holds ([`MAX_OPERATIONAL_DATA_KEYS`]).
///
/// This is the one rule a write of an OpsItem holds its operational data against, so a
/// create and an edit admit exactly the same maps — an edit is held against the map it
/// would leave behind, not against the keys it names.
///
/// A key is held against the reserved prefixes case-insensitively, so no casing of a
/// reserved prefix slips through, but the exemption is EXACT: a key is admitted only when
/// it is spelled as the service spells the key it defines, so a differently-cased spelling
/// is refused rather than stored as data the reads of those keys would never find.
pub fn operational_data_rejection(
    operational_data: &BTreeMap<String, OpsItemDataValue>,
) -> Option<ParameterRejection> {
    if operational_data.len() > MAX_OPERATIONAL_DATA_KEYS {
        return Some(ParameterRejection::new(
            "OperationalData",
            format!(
                "This emulator holds at most {MAX_OPERATIONAL_DATA_KEYS} operational data keys \
                 per OpsItem, and this one would hold {}",
                operational_data.len()
            ),
        ));
    }
    for (key, value) in operational_data {
        let folded = key.to_ascii_lowercase();
        let reserved = RESERVED_OPERATIONAL_DATA_PREFIXES
            .iter()
            .any(|prefix| folded.starts_with(prefix));
        let defined_by_service = SERVICE_OPERATIONAL_DATA_KEYS
            .iter()
            .any(|defined| key == *defined);
        if reserved && !defined_by_service {
            return Some(ParameterRejection::new(
                "OperationalData",
                format!(
                    "Operational data keys can't begin with the following: {}",
                    RESERVED_OPERATIONAL_DATA_PREFIXES.join(", ")
                ),
            ));
        }
        let size = value.value.as_deref().unwrap_or_default().len();
        if size > MAX_OPERATIONAL_DATA_VALUE {
            return Some(ParameterRejection::new(
                "OperationalData",
                format!(
                    "The value of operational data key {key} is {size} bytes: the value has a \
                     maximum size of {MAX_OPERATIONAL_DATA_VALUE} bytes"
                ),
            ));
        }
    }
    None
}

/// Why `ops_item_type` is not an OpsItem type, or `None` when it is one of the types
/// "Systems Manager supports" ([`OPS_ITEM_TYPES`]).
pub fn ops_item_type_rejection(ops_item_type: &str) -> Option<ParameterRejection> {
    (!OPS_ITEM_TYPES.contains(&ops_item_type)).then(|| {
        ParameterRejection::new(
            "OpsItemType",
            format!(
                "OpsItemType {ops_item_type} isn't one of the supported types of OpsItems: {}",
                OPS_ITEM_TYPES.join(", ")
            ),
        )
    })
}

/// Why `severity` is not an OpsItem severity, or `None` when it is one of
/// [`OPS_ITEM_SEVERITIES`].
pub fn severity_rejection(severity: &str) -> Option<ParameterRejection> {
    (!OPS_ITEM_SEVERITIES.contains(&severity)).then(|| {
        ParameterRejection::new(
            "Severity",
            format!(
                "Severity {severity} isn't one of the severity options, which range from {} to {}",
                OPS_ITEM_SEVERITIES.first().copied().unwrap_or_default(),
                OPS_ITEM_SEVERITIES.last().copied().unwrap_or_default()
            ),
        )
    })
}

/// Why `category` is not an OpsItem category, or `None` when it is one of
/// [`OPS_ITEM_CATEGORIES`].
pub fn category_rejection(category: &str) -> Option<ParameterRejection> {
    (!OPS_ITEM_CATEGORIES.contains(&category)).then(|| {
        ParameterRejection::new(
            "Category",
            format!(
                "Category {category} isn't one of the category options: {}",
                OPS_ITEM_CATEGORIES.join(", ")
            ),
        )
    })
}

/// The `OperationalData` map an edit of an OpsItem leaves behind.
///
/// An edit "adds new keys or edits existing key-value pairs of the OperationalData map"
/// (`UpdateOpsItem`'s `OperationalData` doc) and removes the keys `to_delete` names
/// (`OperationalDataToDelete`: "keys that you want to remove from the OperationalData
/// map"), so the result is a PATCH of what the OpsItem already holds rather than a
/// replacement of it: a key neither side names keeps the value the OpsItem holds for it.
///
/// Removals are applied before additions, so a key an edit both deletes and supplies a
/// value for is left carrying that value — the edit named it as data to hold. Deleting a
/// key the map does not hold leaves it absent, so one edit replayed twice leaves the one
/// map.
pub fn merged_operational_data(
    current: &BTreeMap<String, OpsItemDataValue>,
    additions: &BTreeMap<String, OpsItemDataValue>,
    to_delete: &[String],
) -> BTreeMap<String, OpsItemDataValue> {
    let mut merged = current.clone();
    for key in to_delete {
        merged.remove(key);
    }
    for (key, value) in additions {
        merged.insert(key.clone(), value.clone());
    }
    merged
}

/// The dedup string `operational_data` carries, or `None` when it carries none.
///
/// The `/aws/dedup` value is a JSON document with a `dedupString` member, so a value that
/// is not one carries no dedup string at all and is held as ordinary operational data
/// rather than refused.
pub fn dedup_string(operational_data: &BTreeMap<String, OpsItemDataValue>) -> Option<String> {
    let raw = operational_data
        .get(DEDUP_OPERATIONAL_DATA_KEY)?
        .value
        .as_deref()?;
    let parsed: ::serde_json::Value = ::serde_json::from_str(raw).ok()?;
    Some(parsed.get(DEDUP_STRING_MEMBER)?.as_str()?.to_string())
}

/// Mint an OpsItem id in the form the modelled `OpsItemId` pattern requires:
/// `oi-` followed by [`OPS_ITEM_ID_HEX_DIGITS`] lowercase hex digits.
///
/// Through the crate's one minter of a prefixed hex identifier
/// ([`super::minted_prefixed_hex_id`]): the digits are the leading bytes of a digest of
/// the store's own mint counter and the instant of the create, so two OpsItems of one
/// store never carry one id and an id carries nothing from the request that created it.
pub fn mint_ops_item_id(sequence: u64, at: SystemTime) -> String {
    super::minted_prefixed_hex_id(OPS_ITEM_ID_PREFIX, OPS_ITEM_ID_HEX_DIGITS, sequence, at)
}

/// Whether `id` is shaped like an OpsItem id: the form the modelled `OpsItemId` pattern
/// requires (`^(oi)-[0-9a-f]{12}$`), which is the form this store mints
/// ([`mint_ops_item_id`]).
///
/// What tells a page token this service handed out from a value that was never one of its ids
/// ([`super::ops_item_queries`]).
pub fn is_ops_item_id(id: &str) -> bool {
    id.strip_prefix(OPS_ITEM_ID_PREFIX).is_some_and(|digits| {
        digits.len() == OPS_ITEM_ID_HEX_DIGITS
            && digits
                .bytes()
                .all(|digit| matches!(digit, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// One OpsItem as the store holds it.
///
/// Every member `GetOpsItem` reports is here, so a read projects this record
/// ([`OpsItemRecord::to_ops_item`]) rather than recomputing anything: the status, the
/// version, who created it and when all come from what the create persisted.
#[derive(Debug, Clone)]
pub struct OpsItemRecord {
    /// The server-minted id, unique within the store ([`mint_ops_item_id`]).
    pub ops_item_id: String,
    /// The Region whose endpoint this OpsItem was created through, and so the one it is
    /// reached through and the one its ARN names.
    pub region: String,
    /// "A short heading that describes the nature of the OpsItem".
    pub title: String,
    /// "User-defined text that contains information about the OpsItem".
    pub description: String,
    /// "The origin of the OpsItem".
    pub source: String,
    /// One of [`OPS_ITEM_TYPES`].
    pub ops_item_type: String,
    /// One of the modelled `OpsItemStatus` values; [`OPS_ITEM_STATUS_OPEN`] at create.
    pub status: String,
    /// "The importance of this OpsItem in relation to other OpsItems", 1..5.
    pub priority: Option<i32>,
    /// One of [`OPS_ITEM_CATEGORIES`], when the create named one.
    pub category: Option<String>,
    /// One of [`OPS_ITEM_SEVERITIES`], when the create named one.
    pub severity: Option<String>,
    /// The SNS topics "where notifications are sent when this OpsItem is edited".
    pub notifications: Vec<OpsItemNotification>,
    /// "One or more OpsItems that share something in common with the current OpsItem".
    pub related_ops_items: Vec<RelatedOpsItem>,
    /// The custom key-value data the create supplied, ordered so two reads of one OpsItem
    /// report it the same way.
    pub operational_data: BTreeMap<String, OpsItemDataValue>,
    /// The dedup string the create's `/aws/dedup` value carried ([`dedup_string`]), or
    /// `None` when it carried none. Read out of the operational data ONCE, at create, so a
    /// later create's collision check is a string comparison per held OpsItem rather than
    /// a re-parse of every one of their operational-data documents.
    pub dedup: Option<String>,
    /// "The time a runbook workflow started", as the create reported it.
    pub actual_start_time: Option<::aws_smithy_types::DateTime>,
    /// "The time a runbook workflow ended", as the create reported it.
    pub actual_end_time: Option<::aws_smithy_types::DateTime>,
    /// "The time specified in a change request for a runbook workflow to start".
    pub planned_start_time: Option<::aws_smithy_types::DateTime>,
    /// "The time specified in a change request for a runbook workflow to end".
    pub planned_end_time: Option<::aws_smithy_types::DateTime>,
    /// The tags the create assigned, keyed by tag key.
    pub tags: BTreeMap<String, String>,
    /// "The ARN of the Amazon Web Services account that created the OpsItem".
    pub created_by: String,
    /// "The date and time the OpsItem was created".
    pub created_time: SystemTime,
    /// "The ARN of the Amazon Web Services account that last updated the OpsItem" — the
    /// creator, until an edit says otherwise.
    pub last_modified_by: String,
    /// "The date and time the OpsItem was last updated".
    pub last_modified_time: SystemTime,
    /// "The version of this OpsItem. Each time the OpsItem is edited the version number
    /// increments by one" — the token the last write of this record left
    /// ([`minted_ops_item_version`]).
    pub version: u64,
    /// The instant this OpsItem's delete is owed to have settled at, once one has been
    /// asked for: a delete is eventually consistent ([`OPS_ITEM_DELETE_SETTLES_AFTER`]), so
    /// the record is still reported until then and is removed at it
    /// ([`ComAmazonawsSsmAmazonssmState::settle_ops_item_deletions_due`]). `None` for an
    /// OpsItem no delete has been asked for.
    pub delete_due: Option<SystemTime>,
}

impl OpsItemRecord {
    /// This OpsItem's ARN: `arn:aws:ssm:<region>:<account>:opsitem/<id>`.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            ARN_SERVICE,
            &self.region,
            &format!("{ARN_OPS_ITEM_RESOURCE_PREFIX}{}", self.ops_item_id),
        )
    }

    /// Whether this OpsItem is still being worked, and so still deduplicates a create
    /// that carries its dedup string ([`OPS_ITEM_TERMINAL_STATUSES`]).
    pub fn is_open(&self) -> bool {
        !OPS_ITEM_TERMINAL_STATUSES.contains(&self.status.as_str())
    }

    /// Whether a delete of this OpsItem has been asked for and has not settled yet, which
    /// is the state no edit of it is served in: "the specified OpsItem is in the process of
    /// being deleted".
    pub fn is_deleting(&self) -> bool {
        self.delete_due.is_some()
    }

    /// The modelled `OpsItem` for this record — every member projected from what was
    /// persisted, and the ones the store holds nothing for left unset rather than
    /// invented.
    pub fn to_ops_item(&self) -> OpsItem {
        OpsItem {
            created_by: Some(self.created_by.clone()),
            ops_item_type: Some(self.ops_item_type.clone()),
            created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
            description: Some(self.description.clone()),
            last_modified_by: Some(self.last_modified_by.clone()),
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.last_modified_time)),
            notifications: Some(self.notifications.clone()),
            priority: self.priority,
            related_ops_items: Some(self.related_ops_items.clone()),
            status: Some(self.status.clone()),
            ops_item_id: Some(self.ops_item_id.clone()),
            version: Some(self.version.to_string()),
            title: Some(self.title.clone()),
            source: Some(self.source.clone()),
            operational_data: Some(
                self.operational_data
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            category: self.category.clone(),
            severity: self.severity.clone(),
            actual_start_time: self.actual_start_time,
            actual_end_time: self.actual_end_time,
            planned_start_time: self.planned_start_time,
            planned_end_time: self.planned_end_time,
            ops_item_arn: Some(self.arn()),
        }
    }

    /// The modelled `OpsItemSummary` for this record: the members a QUERY reports, projected
    /// from the same persisted record the single-OpsItem read projects
    /// ([`Self::to_ops_item`]), so a listing and a read never disagree about one OpsItem.
    ///
    /// `OpsItemSummary` carries no `Description`, `Notifications`, `RelatedOpsItems`, `Version`
    /// or `OpsItemArn`: a summary is what the model says it is, and those are read through
    /// `GetOpsItem`.
    pub fn to_summary(&self) -> OpsItemSummary {
        OpsItemSummary {
            created_by: Some(self.created_by.clone()),
            created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
            last_modified_by: Some(self.last_modified_by.clone()),
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.last_modified_time)),
            priority: self.priority,
            source: Some(self.source.clone()),
            status: Some(self.status.clone()),
            ops_item_id: Some(self.ops_item_id.clone()),
            title: Some(self.title.clone()),
            operational_data: Some(
                self.operational_data
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
            category: self.category.clone(),
            severity: self.severity.clone(),
            ops_item_type: Some(self.ops_item_type.clone()),
            actual_start_time: self.actual_start_time,
            actual_end_time: self.actual_end_time,
            planned_start_time: self.planned_start_time,
            planned_end_time: self.planned_end_time,
        }
    }
}

impl ComAmazonawsSsmAmazonssmState {
    /// The OpsItem `ops_item_id` names in `region`, or `None` when that Region holds
    /// none.
    pub fn ops_item(&self, region: &str, ops_item_id: &str) -> Option<&OpsItemRecord> {
        self.ops_items
            .get(&(region.to_string(), ops_item_id.to_string()))
    }

    /// The same OpsItem, to write it — its tags, say.
    pub fn ops_item_mut(&mut self, region: &str, ops_item_id: &str) -> Option<&mut OpsItemRecord> {
        self.ops_items
            .get_mut(&(region.to_string(), ops_item_id.to_string()))
    }

    /// The OpsItem `region` already holds open under `dedup`, or `None` when it holds
    /// none: an OpsItem that has been resolved or closed is finished work, so a create
    /// carrying its dedup string opens a new OpsItem rather than colliding with it.
    pub fn open_ops_item_with_dedup(&self, region: &str, dedup: &str) -> Option<&OpsItemRecord> {
        self.ops_items
            .values()
            .filter(|held| held.region == region && held.is_open())
            .find(|held| held.dedup.as_deref() == Some(dedup))
    }

    /// An OpsItem id no OpsItem of this store carries, and the mint counter advanced past
    /// it ([`mint_ops_item_id`]).
    pub fn next_ops_item_id(&mut self, at: SystemTime) -> String {
        loop {
            self.ops_item_sequence += 1;
            let candidate = mint_ops_item_id(self.ops_item_sequence, at);
            if !self
                .ops_items
                .keys()
                .any(|(_, held)| held.as_str() == candidate)
            {
                return candidate;
            }
        }
    }

    /// Whether the store already holds as many OpsItems as it may ([`MAX_OPS_ITEMS`]), and
    /// so cannot take a new one. Checked before a create mints an id, so a create the
    /// store would refuse does not advance the mint counter.
    pub fn ops_items_at_capacity(&self) -> bool {
        self.ops_items.len() >= MAX_OPS_ITEMS
    }

    /// Store `record` as its Region's OpsItem of that id.
    ///
    /// Refuses with the bound ([`MAX_OPS_ITEMS`]) when storing it would take the store past
    /// it, leaving the store untouched: this is the one write path, so the bound holds
    /// however a caller reached it. Replacing a record the store already holds is not
    /// growth and is allowed at capacity.
    pub fn insert_ops_item(&mut self, record: OpsItemRecord) -> Result<(), usize> {
        let key = (record.region.clone(), record.ops_item_id.clone());
        if !self.ops_items.contains_key(&key) && self.ops_items_at_capacity() {
            return Err(MAX_OPS_ITEMS);
        }
        self.ops_items.insert(key, record);
        Ok(())
    }

    /// Ask for the OpsItem `ops_item_id` names in `region` to be deleted, reporting whether
    /// that Region held one.
    ///
    /// "Deleting an OpsItem is irreversible. You can't restore a deleted OpsItem", and the
    /// delete is eventually consistent: the record is marked as owed removal at
    /// `at` + [`OPS_ITEM_DELETE_SETTLES_AFTER`] and is still reported until then, which is
    /// what a read made straight after the delete is served. The removal itself is
    /// materialised from that due instant ([`Self::settle_ops_item_deletions_due`]).
    ///
    /// Idempotent in both directions: "the system doesn't throw an exception if you
    /// repeatedly call this operation for the same OpsItem. If the first call is successful,
    /// all additional calls return the same successful response as the first call" — so a
    /// repeated delete keeps the due instant the FIRST one set rather than putting the
    /// removal off, and a delete of an id the Region holds no OpsItem under marks nothing.
    pub fn delete_ops_item(&mut self, region: &str, ops_item_id: &str, at: SystemTime) -> bool {
        let due = at + OPS_ITEM_DELETE_SETTLES_AFTER;
        match self.ops_item_mut(region, ops_item_id) {
            Some(record) => {
                record.delete_due = Some(record.delete_due.map_or(due, |held| held.min(due)));
                true
            }
            None => false,
        }
    }

    /// Remove every OpsItem whose delete was already owed to have settled at `now`,
    /// together with the related-item associations held against it: an association is
    /// reached only through the OpsItem it belongs to, so one of an OpsItem nothing holds
    /// any more is a related item of nothing.
    ///
    /// The removal was due at that instant whether or not anybody called, so this is what
    /// a read materialises before it resolves anything against the store
    /// ([`ComAmazonawsSsmAmazonssmState::materialize_due`]).
    pub fn settle_ops_item_deletions_due(&mut self, now: SystemTime) {
        let settled: Vec<(String, String)> = self
            .ops_items
            .iter()
            .filter(|(_, held)| held.delete_due.is_some_and(|due| due <= now))
            .map(|(key, _)| key.clone())
            .collect();
        for (region, ops_item_id) in settled {
            self.ops_items.remove(&(region.clone(), ops_item_id.clone()));
            self.ops_item_related_items
                .retain(|(held_region, held_ops_item, _), _| {
                    held_region != &region || held_ops_item != &ops_item_id
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::*;
    use crate::ops::types::OPS_ITEM_STATUS_VALUES;

    /// A record of `id` in `region`, carrying only what the store keys and bounds it by.
    fn record(region: &str, id: &str) -> OpsItemRecord {
        OpsItemRecord {
            ops_item_id: id.to_string(),
            region: region.to_string(),
            title: "An OpsItem".to_string(),
            description: "what it is about".to_string(),
            source: "tests".to_string(),
            ops_item_type: DEFAULT_OPS_ITEM_TYPE.to_string(),
            status: OPS_ITEM_STATUS_OPEN.to_string(),
            priority: None,
            category: None,
            severity: None,
            notifications: Vec::new(),
            related_ops_items: Vec::new(),
            operational_data: BTreeMap::new(),
            dedup: None,
            actual_start_time: None,
            actual_end_time: None,
            planned_start_time: None,
            planned_end_time: None,
            tags: BTreeMap::new(),
            created_by: "tests".to_string(),
            created_time: UNIX_EPOCH,
            last_modified_by: "tests".to_string(),
            last_modified_time: UNIX_EPOCH,
            version: minted_ops_item_version(UNIX_EPOCH, None),
            delete_due: None,
        }
    }

    /// The store's one write path refuses the OpsItem past its bound, whatever Region it
    /// would be stored under, and replacing one it already holds is still allowed there.
    #[test]
    fn the_store_refuses_an_ops_item_past_its_bound() {
        let mut store = ComAmazonawsSsmAmazonssmState::default();
        for sequence in 0..MAX_OPS_ITEMS {
            let region = if sequence % 2 == 0 {
                "us-west-2"
            } else {
                "eu-central-1"
            };
            let outcome = store.insert_ops_item(record(region, &format!("oi-{sequence:012x}")));
            assert_eq!(
                outcome,
                Ok(()),
                "case {sequence}: a store holding {} must take another, got {outcome:?}",
                store.ops_items.len()
            );
        }
        assert!(
            store.ops_items_at_capacity(),
            "expected a store of {} to be at its {MAX_OPS_ITEMS} bound",
            store.ops_items.len()
        );
        // One past the bound, in a Region the store already holds OpsItems in and in one
        // it holds none in: a caller-chosen Region does not widen the bound.
        for region in ["us-west-2", "ap-southeast-2"] {
            let outcome = store.insert_ops_item(record(region, "oi-ffffffffffff"));
            assert_eq!(
                outcome,
                Err(MAX_OPS_ITEMS),
                "case {region}: expected the bound to be reported, got {outcome:?}"
            );
            assert_eq!(
                store.ops_items.len(),
                MAX_OPS_ITEMS,
                "case {region}: a refused insert stores nothing, got {} held",
                store.ops_items.len()
            );
        }
        // Replacing a record the store already holds is not growth.
        let mut replacement = record("us-west-2", "oi-000000000000");
        replacement.title = "The same OpsItem, edited".to_string();
        let outcome = store.insert_ops_item(replacement);
        assert_eq!(
            outcome,
            Ok(()),
            "case replacement: a store at its bound may still replace a record it holds, got \
             {outcome:?}"
        );
        assert_eq!(
            store
                .ops_item("us-west-2", "oi-000000000000")
                .map(|held| held.title.as_str()),
            Some("The same OpsItem, edited"),
            "case replacement: expected the replacement to be stored"
        );
    }

    /// A minted id satisfies the modelled `OpsItemId` pattern, and two mints of one
    /// counter sequence differ.
    #[test]
    fn a_minted_id_is_ops_item_shaped_and_fresh() {
        let at = UNIX_EPOCH + ::std::time::Duration::from_nanos(1_700_000_000_123_456_789);
        let first = mint_ops_item_id(1, at);
        let second = mint_ops_item_id(2, at);
        for id in [&first, &second] {
            let digits = id
                .strip_prefix(OPS_ITEM_ID_PREFIX)
                .unwrap_or_else(|| panic!("case {id}: an id starts with {OPS_ITEM_ID_PREFIX}"));
            assert_eq!(
                digits.len(),
                OPS_ITEM_ID_HEX_DIGITS,
                "case {id}: expected {OPS_ITEM_ID_HEX_DIGITS} hex digits, got {digits:?}"
            );
            assert!(
                digits.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()),
                "case {id}: expected lowercase hex digits, got {digits:?}"
            );
        }
        assert_ne!(
            first, second,
            "two sequences must mint two ids, got {first} twice"
        );
    }

    /// `Open` is a value of the modelled status enum, as are the terminal ones.
    #[test]
    fn the_statuses_named_here_are_modelled_values() {
        for status in [OPS_ITEM_STATUS_OPEN]
            .into_iter()
            .chain(OPS_ITEM_TERMINAL_STATUSES.iter().copied())
        {
            assert!(
                OPS_ITEM_STATUS_VALUES.contains(&status),
                "case {status}: expected a modelled OpsItemStatus value, got {status:?}"
            );
        }
    }

    /// A `/aws/dedup` value is read for its `dedupString` member, and a value that is not
    /// one carries no dedup string.
    #[test]
    fn a_dedup_string_is_read_out_of_the_dedup_key() {
        let cases: &[(&str, Option<&str>)] = &[
            (r#"{"dedupString": "disk-full"}"#, Some("disk-full")),
            (r#"{"dedupString": "/host/a:/dev/sda1"}"#, Some("/host/a:/dev/sda1")),
            (r#"{"other": "disk-full"}"#, None),
            ("not json at all", None),
            (r#"{"dedupString": 7}"#, None),
        ];
        for (raw, expected) in cases {
            let data = BTreeMap::from([(
                DEDUP_OPERATIONAL_DATA_KEY.to_string(),
                OpsItemDataValue {
                    value: Some((*raw).to_string()),
                    r#type: None,
                },
            )]);
            let actual = dedup_string(&data);
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {raw:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A minted version token is the instant of the write in milliseconds, and a write of
    /// an OpsItem that already carries one leaves a STRICTLY greater token however close
    /// together the two writes are — including two writes of the one millisecond.
    #[test]
    fn a_minted_version_advances_past_the_one_it_replaces() {
        let at = UNIX_EPOCH + ::std::time::Duration::from_millis(1_762_240_284_123);
        let cases: &[(&str, Option<u64>, u64)] = &[
            ("a create", None, 1_762_240_284_123),
            // An edit a whole millisecond later than the write before it: the clock alone
            // already orders the two.
            ("an edit of an older version", Some(1_762_240_284_122), 1_762_240_284_123),
            // An edit in the SAME millisecond as the write before it, and one the clock
            // cannot order at all: the token is advanced past the one held instead.
            ("an edit in the same millisecond", Some(1_762_240_284_123), 1_762_240_284_124),
            ("an edit against a newer version", Some(1_900_000_000_000), 1_900_000_000_001),
        ];
        for (case, previous, expected) in cases {
            let actual = minted_ops_item_version(at, *previous);
            assert_eq!(
                actual, *expected,
                "case {case}: expected version {expected}, got {actual}"
            );
        }
    }

    /// A delete settles at its due instant: until then the OpsItem is still reported and is
    /// marked as being deleted, at it the record and the related items held against it are
    /// gone, every other OpsItem and its related items are untouched, and a repeated delete
    /// settles when the FIRST one was due rather than putting the removal off.
    #[test]
    fn a_delete_settles_at_its_due_instant() {
        let mut store = ComAmazonawsSsmAmazonssmState::default();
        let related = |region: &str, ops_item_id: &str| {
            crate::state::ops_item_related_items::RelatedItemRecord {
                association_id: format!("{ops_item_id}-association"),
                ops_item_id: ops_item_id.to_string(),
                region: region.to_string(),
                resource_type: "AWS::SSM::Document".to_string(),
                association_type: "RelatesTo".to_string(),
                resource_uri: format!("arn:aws:ssm:{region}:123456789012:document/{ops_item_id}"),
                created_by: "tests".to_string(),
                created_time: UNIX_EPOCH,
                last_modified_by: "tests".to_string(),
                last_modified_time: UNIX_EPOCH,
            }
        };
        for (region, id) in [
            ("us-west-2", "oi-00000000000a"),
            ("us-west-2", "oi-00000000000b"),
            ("eu-central-1", "oi-00000000000a"),
        ] {
            store
                .insert_ops_item(record(region, id))
                .unwrap_or_else(|limit| panic!("case {region}/{id}: the store took {limit}"));
            store
                .insert_related_item(related(region, id))
                .unwrap_or_else(|limit| panic!("case {region}/{id}: the store took {limit}"));
        }

        let asked_at = UNIX_EPOCH + ::std::time::Duration::from_secs(1_700_000_000);
        assert!(
            store.delete_ops_item("us-west-2", "oi-00000000000a", asked_at),
            "case a held OpsItem: expected the delete to report the record it marked"
        );
        // Every instant before the delete is due: the OpsItem is still there, and says it
        // is being deleted.
        for before in [
            asked_at,
            asked_at + OPS_ITEM_DELETE_SETTLES_AFTER / 2,
            asked_at + OPS_ITEM_DELETE_SETTLES_AFTER - ::std::time::Duration::from_millis(1),
        ] {
            store.settle_ops_item_deletions_due(before);
            let held = store.ops_item("us-west-2", "oi-00000000000a");
            assert!(
                held.is_some_and(super::OpsItemRecord::is_deleting),
                "case {before:?}: expected an OpsItem being deleted, got {held:?}"
            );
        }
        // A repeated delete does not put the removal off.
        assert!(
            store.delete_ops_item(
                "us-west-2",
                "oi-00000000000a",
                asked_at + OPS_ITEM_DELETE_SETTLES_AFTER
            ),
            "case a repeated delete: expected it to report the record it marked"
        );

        store.settle_ops_item_deletions_due(asked_at + OPS_ITEM_DELETE_SETTLES_AFTER);
        assert!(
            store.ops_item("us-west-2", "oi-00000000000a").is_none(),
            "case the due instant: expected the OpsItem to be gone"
        );
        assert_eq!(
            store
                .related_items_in_region("us-west-2")
                .map(|held| held.ops_item_id.clone())
                .collect::<Vec<String>>(),
            vec!["oi-00000000000b".to_string()],
            "case the due instant: only the deleted OpsItem's related items go"
        );
        assert!(
            store.ops_item("eu-central-1", "oi-00000000000a").is_some(),
            "case another Region: an OpsItem of another Region is untouched"
        );
        assert_eq!(
            store.related_items_in_region("eu-central-1").count(),
            1,
            "case another Region: its related items are untouched"
        );
        assert!(
            !store.delete_ops_item("us-west-2", "oi-00000000000a", asked_at),
            "case a delete of a settled id: expected it to report that it marked nothing"
        );
        assert!(
            !store.delete_ops_item("us-west-2", "oi-ffffffffffff", asked_at),
            "case an id nothing is held under: expected it to report that it marked nothing"
        );
    }
}
