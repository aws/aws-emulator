// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime service.
//!
//! The service is the OnlineStore data plane of SageMaker Feature Store: every
//! operation reads or writes the one store modelled here. The runtime API carries no
//! feature-group DEFINITION — that lives in the SageMaker control plane — so the name a
//! request carries is resolved, and the two names the definition supplies (the group's
//! record-identifier feature and its event-time feature) are read, through the SHARED
//! feature-group directory the control plane writes: [`resolve_group`], over
//! [`::service_common::feature_store_groups::FeatureGroupDirectory`]. A name the control
//! plane never created resolves to nothing and the call is refused; nothing here guesses
//! a definition from the records a caller happens to write.
//!
//! The store is held in memory for the life of the process, so its three growth sites
//! are bounded by a maximum count: [`MAX_FEATURE_GROUPS`] groups, each holding at most
//! [`MAX_RECORDS_PER_GROUP`] records, each holding at most [`MAX_FEATURES_PER_RECORD`]
//! feature values. A write past any of the three is refused (the handler answers the
//! modelled `ServiceUnavailable`) rather than growing without limit, and a record written
//! under a TTL — the one its own write named, or the default its feature group configures
//! ([`effective_ttl_seconds`]) — is additionally evicted once its expiry is due.
//!
//! The group's OFFLINE store is not held here at all: it is a location in Amazon S3 the
//! control plane configured, which each ingestion REPLICATES one object into
//! ([`offline_store`]).

use std::collections::BTreeMap;
use std::collections::HashMap;

use aws_smithy_types::date_time::{DateTime, Format};

/// The OfflineStore side of an ingestion: the Amazon S3 object a replicated record lands
/// as, and the write that puts it there. The ONLINE store is what the rest of this module
/// holds; a feature group's offline store is a location in another service, which is why
/// its own slice lives beside this one.
pub mod offline_store;

/// The most feature groups one emulator keeps an OnlineStore for.
///
/// A capacity bound, not an AWS quota: the store is per-process memory that grows with
/// each distinct caller-supplied `FeatureGroupName`, so it needs a maximum count.
pub const MAX_FEATURE_GROUPS: usize = 1_000;

/// The most records one feature group's OnlineStore keeps.
///
/// Same reason as [`MAX_FEATURE_GROUPS`]: `PutRecord` adds an entry per distinct record
/// identifier, and a caller that passes no `TtlDuration` would otherwise grow the group
/// without limit.
pub const MAX_RECORDS_PER_GROUP: usize = 10_000;

/// The most feature values one record of a feature group's OnlineStore keeps.
///
/// Same reason as [`MAX_RECORDS_PER_GROUP`], for the one dimension `PutRecord` cannot
/// grow: a `PutRecord` replaces a record's feature values wholesale, so its residency is
/// bounded by one request, but `UpdateRecord` is a PARTIAL update — a feature name the
/// stored record does not carry yet is appended to it — so successive calls naming fresh
/// names would otherwise grow one record without limit, and each `ValueAsString` is
/// modelled `len 0..358400`. A merge past this bound is refused rather than taken.
pub const MAX_FEATURES_PER_RECORD: usize = 1_000;

/// The page size `ListRecords` uses when the caller names no `MaxResults`.
///
/// The model constrains `MaxResults` to `range 1..100`, so 100 is the largest page the
/// API lets a caller ask for; using it as the default keeps a response from being linear
/// in the whole store.
pub const DEFAULT_LIST_PAGE_SIZE: usize = 100;

/// Per-service state for com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSagemakerfeaturestoreruntimeAmazonsagemakerfeaturestoreruntimeState {
    /// OnlineStore per feature group, keyed by [`group_key`] (so an ARN and the bare
    /// name of one group reach one store). At most [`MAX_FEATURE_GROUPS`] entries.
    groups: HashMap<String, FeatureGroupStore>,
}

impl ComAmazonawsSagemakerfeaturestoreruntimeAmazonsagemakerfeaturestoreruntimeState {
    /// The OnlineStore of `name_or_arn`, or `None` when no record was ever ingested
    /// into that feature group. Expired records are dropped first, so a caller never
    /// sees a record whose `ExpiresAt` has passed.
    pub fn group_mut(&mut self, name_or_arn: &str) -> Option<&mut FeatureGroupStore> {
        let group = self.groups.get_mut(group_key(name_or_arn))?;
        group.purge_expired(now_secs());
        Some(group)
    }

    /// The OnlineStore of `name_or_arn`, created empty when this is the first record
    /// ingested into that feature group. The runtime API has no "create feature group"
    /// call, so a group's online store comes into existence with its first `PutRecord`.
    ///
    /// `None` when the group is not known yet and the emulator already holds
    /// [`MAX_FEATURE_GROUPS`] of them: the caller refuses the write rather than growing
    /// the store without limit.
    pub fn group_or_create(&mut self, name_or_arn: &str) -> Option<&mut FeatureGroupStore> {
        let key = group_key(name_or_arn);
        if !self.groups.contains_key(key) && self.groups.len() >= MAX_FEATURE_GROUPS {
            return None;
        }
        let group = self.groups.entry(key.to_owned()).or_default();
        group.purge_expired(now_secs());
        Some(group)
    }
}

/// One feature group's OnlineStore.
///
/// The group's DEFINITION is not here: it is the control plane's, read through
/// [`resolve_group`]. This holds only what the data plane itself wrote.
#[derive(Debug, Default)]
pub struct FeatureGroupStore {
    /// Records by `RecordIdentifierValueAsString`, at most [`MAX_RECORDS_PER_GROUP`] of
    /// them. A `BTreeMap` so `ListRecords` enumerates in one deterministic ASCENDING
    /// identifier order — which is what lets a page token name the identifier to resume
    /// from rather than a positional index that insertions would shift.
    records: BTreeMap<String, StoredRecord>,
    /// A LOWER BOUND on the earliest `expires_at_secs` any record holds: no record
    /// expires before it. A read path with nothing due therefore skips the expiry scan
    /// entirely, and a bound left behind by a removed record only costs one scan, which
    /// recomputes it.
    next_expiry_secs: Option<f64>,
}

impl FeatureGroupStore {
    /// Hard-delete every record whose `ExpiresAt` is at or before `now_secs`.
    ///
    /// This is the TTL the model documents — "Time to live duration, where the record
    /// is hard deleted after the expiration time is reached" — so the removal is owed
    /// by the clock and is materialised lazily on the next call that reads the group,
    /// rather than by a timer. Nothing is scanned while no expiry is due.
    pub fn purge_expired(&mut self, now_secs: f64) {
        if self.next_expiry_secs.is_none_or(|due| due > now_secs) {
            return;
        }
        self.records
            .retain(|_, record| record.expires_at_secs.is_none_or(|at| at > now_secs));
        self.recompute_next_expiry();
    }

    /// The records of this group, in ascending identifier order.
    pub fn records(&self) -> impl Iterator<Item = &StoredRecord> {
        self.records.values()
    }

    /// The record `identifier` names, whether or not it is soft-deleted.
    pub fn record(&self, identifier: &str) -> Option<&StoredRecord> {
        self.records.get(identifier)
    }

    /// The record `identifier` names, if it is present and not soft-deleted.
    pub fn live_record(&self, identifier: &str) -> Option<&StoredRecord> {
        self.records
            .get(identifier)
            .filter(|record| !record.soft_deleted)
    }

    /// Store `record`, over-writing any record with the same identifier.
    ///
    /// `false` — nothing stored — when the record is new and the group already holds
    /// [`MAX_RECORDS_PER_GROUP`].
    pub fn insert_record(&mut self, record: StoredRecord) -> bool {
        let is_new = !self.records.contains_key(&record.identifier);
        if is_new && self.records.len() >= MAX_RECORDS_PER_GROUP {
            return false;
        }
        if let Some(at) = record.expires_at_secs {
            self.next_expiry_secs = Some(self.next_expiry_secs.map_or(at, |earliest| earliest.min(at)));
        }
        self.records.insert(record.identifier.clone(), record);
        true
    }

    /// Offer a prepared record to this group's online store: the one INGESTION every
    /// write of a whole record performs.
    ///
    /// "If a new record's EventTime is greater, the new record is written to both the
    /// OnlineStore and OfflineStore. Otherwise, the record is a historic record and it is
    /// written only to the OfflineStore": this is the ONLINE store, so a posted record
    /// whose event time is not later than the stored record's leaves that record standing
    /// ([`Ingested::Historic`]) while the caller still replicates it into the group's
    /// offline store ([`offline_store`]). A record the group does not hold yet is
    /// stored unless the group is full ([`Ingested::AtCapacity`]).
    pub fn ingest(&mut self, prepared: PreparedRecord) -> Ingested {
        if let (Some(posted), Some(stored)) = (
            prepared.event_time_secs,
            self.record(&prepared.record.identifier)
                .and_then(StoredRecord::event_time_secs),
        ) {
            if posted <= stored {
                return Ingested::Historic;
            }
        }
        if self.insert_record(prepared.record) {
            Ingested::Stored
        } else {
            Ingested::AtCapacity
        }
    }

    /// Remove the record entirely (`HardDelete`).
    pub fn remove_record(&mut self, identifier: &str) {
        self.records.remove(identifier);
    }

    /// Leave the `SoftDelete` tombstone the model describes: "a record with the same
    /// RecordIdentifer as the original, but with `is_deleted` value set to True,
    /// `EventTime` set to the delete input EventTime, and other feature values set to
    /// null."
    ///
    /// The two features the tombstone KEEPS are the group's own
    /// `RecordIdentifierFeatureName` and `EventTimeFeatureName`, which the caller reads
    /// from the group's definition ([`resolve_group`]) and passes in: this store holds no
    /// definition of its own, and a group that resolves always has both names.
    pub fn soft_delete_record(
        &mut self,
        identifier: &str,
        event_time: &str,
        identifier_feature: &str,
        event_time_feature: &str,
    ) {
        if let Some(record) = self.records.get_mut(identifier) {
            record.soft_deleted = true;
            record.event_time = Some(event_time.to_owned());
            record.expires_at = None;
            record.expires_at_secs = None;
            record.ttl_secs = None;
            record.features.retain(|feature| {
                identifier_feature == feature.feature_name
                    || event_time_feature == feature.feature_name
            });
            for feature in &mut record.features {
                if event_time_feature == feature.feature_name {
                    feature.value_as_string = Some(event_time.to_owned());
                    feature.value_as_string_list = None;
                }
            }
        }
    }

    /// Re-derive the expiry bound from the records that remain.
    fn recompute_next_expiry(&mut self) {
        self.next_expiry_secs = self
            .records
            .values()
            .filter_map(|record| record.expires_at_secs)
            .fold(None, |earliest: Option<f64>, at| {
                Some(earliest.map_or(at, |e| e.min(at)))
            });
    }
}

/// One record of a feature group's OnlineStore.
#[derive(Debug, Clone, Default)]
pub struct StoredRecord {
    /// `RecordIdentifierValueAsString` — the value of the group's record-identifier
    /// feature in this record.
    pub identifier: String,
    /// Every feature value of the record, in the order it was written. A `PutRecord`
    /// is a full over-write, so this is replaced wholesale.
    pub features: Vec<StoredFeatureValue>,
    /// The record's event time, exactly as the event-time feature carried it.
    pub event_time: Option<String>,
    /// `EventTime + TtlDuration`, as an ISO-8601 string; absent when the record is under
    /// no TTL at all ([`effective_ttl_seconds`]) or carries no parsable event time.
    pub expires_at: Option<String>,
    /// The same instant as [`Self::expires_at`] in epoch seconds, for the expiry
    /// comparison, so no handler re-parses the string.
    pub expires_at_secs: Option<f64>,
    /// The record's `TtlDuration` in SECONDS — the duration the write that stored it was
    /// under, which is its own when it named one and otherwise its feature group's
    /// default ([`effective_ttl_seconds`]) — kept so a later call that moves the record's
    /// `EventTime` can re-derive `ExpiresAt = EventTime + TtlDuration` from it.
    /// `UpdateRecord` documents exactly that: "If you omit this parameter, the record's
    /// existing TTL setting remains unchanged", and the setting is the DURATION, not the
    /// instant it happened to fall due at.
    pub ttl_secs: Option<f64>,
    /// Set by `DeleteRecord` in `SoftDelete` mode: the record keeps its place in the
    /// store but is no longer retrievable by `GetRecord` or `BatchGetRecord`.
    pub soft_deleted: bool,
}

impl StoredRecord {
    /// The record's event time in epoch seconds, or `None` when it has none or it is
    /// in neither of the two formats a Feature Store event time can take.
    pub fn event_time_secs(&self) -> Option<f64> {
        self.event_time.as_deref().and_then(parse_event_time_secs)
    }

    /// Re-derive `ExpiresAt = EventTime + TtlDuration` on this record from its own
    /// [`Self::event_time`] and [`Self::ttl_secs`], so the stored instant and the two
    /// values it is computed from can never disagree.
    ///
    /// Both members go to `None` when the record carries no TTL duration or no parsable
    /// event time: there is then no expiry the clock owes.
    pub fn recompute_expiry(&mut self) {
        let at = self
            .ttl_secs
            .zip(self.event_time_secs())
            .map(|(ttl, event_time)| event_time + ttl);
        self.expires_at = at.and_then(|at| iso8601(at).ok());
        self.expires_at_secs = at;
    }

    /// This record's features, restricted to `feature_names` when the caller named
    /// any, in the order the record stores them.
    ///
    /// `FeatureNames` is documented as "List of names of Features to be retrieved. If
    /// not specified, the latest value for all the Features are returned", so an
    /// absent list projects everything and a present one projects exactly what it
    /// names. A name the record does not carry contributes nothing and is not an
    /// error: the runtime API carries no feature-group definition, so a feature the
    /// group declares but this record never set is indistinguishable here from a name
    /// the group does not declare at all.
    pub fn project(&self, feature_names: Option<&[String]>) -> Vec<crate::ops::types::FeatureValue> {
        self.features
            .iter()
            .filter(|feature| {
                feature_names.is_none_or(|names| names.contains(&feature.feature_name))
            })
            .map(StoredFeatureValue::to_feature_value)
            .collect()
    }
}

/// What became of a prepared record offered to a feature group's online store
/// ([`FeatureGroupStore::ingest`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingested {
    /// Stored, over-writing any record the group held under the same identifier.
    Stored,
    /// Not stored: the posted record is historic — its event time is not later than
    /// that of the record the group already holds — so only the OfflineStore takes it
    /// ([`offline_store`]). Not a failure.
    Historic,
    /// Not stored: the record is new and the group already holds
    /// [`MAX_RECORDS_PER_GROUP`] of them.
    AtCapacity,
}

/// One whole-record write, read from a request and ready to be committed to a feature
/// group's online store.
///
/// A record ingestion is the same write whether it arrives on its own or as one entry of
/// a batch — "List of FeatureValues to be inserted. This will be a full over-write." —
/// so [`prepare_record`] states those rules once and both callers commit what it hands
/// back through [`FeatureGroupStore::ingest`].
#[derive(Debug, Clone)]
pub struct PreparedRecord {
    /// The record as it would be stored, with `ExpiresAt` already derived.
    record: StoredRecord,
    /// The posted event time in epoch seconds, which is what orders the write against
    /// the record the group already holds.
    event_time_secs: Option<f64>,
}

impl PreparedRecord {
    /// The value of the group's record-identifier feature in this record, i.e. the
    /// record the write identifies.
    pub fn identifier(&self) -> &str {
        &self.record.identifier
    }

    /// `ExpiresAt = EventTime + TtlDuration`, as the ISO-8601 string a read reports,
    /// when the write carries a TTL duration and a parsable event time.
    pub fn expires_at(&self) -> Option<&str> {
        self.record.expires_at.as_deref()
    }
}

/// Read one posted record against the DEFINITION of the feature group it is written to.
///
/// The two names the record is read through are the group's own, as the control plane
/// recorded them — its `RecordIdentifierFeatureName` and its `EventTimeFeatureName` —
/// never the order or the names of the features one request happens to post. The TTL the
/// record is stored under is the group's too whenever the write names none
/// ([`effective_ttl_seconds`]). `Err` is
/// the refusal message for a record this data plane cannot read: a caller answers it
/// with its modelled `ValidationError`, or — in a batch — as that entry's own error.
pub fn prepare_record(
    definition: &::service_common::feature_store_groups::FeatureGroupRecord,
    record: &[crate::ops::types::FeatureValue],
    ttl_duration: Option<&crate::ops::types::TtlDuration>,
) -> Result<PreparedRecord, String> {
    // `Record` is modelled `len 1..` and refused at the boundary; read it fallibly all
    // the same, so a caller reached with an empty list refuses it instead of storing a
    // record with no feature values.
    if record.is_empty() {
        return Err("Record must carry at least one FeatureValue".to_owned());
    }

    let identifier_feature_name = &definition.record_identifier_feature_name;
    let identifier = match record
        .iter()
        .find(|feature| feature.feature_name == *identifier_feature_name)
    {
        Some(crate::ops::types::FeatureValue {
            value_as_string: Some(value),
            ..
        }) => value.clone(),
        Some(_) => {
            return Err(format!(
                "the record identifier feature '{identifier_feature_name}' must carry \
                 ValueAsString, not ValueAsStringList"
            ))
        }
        None => {
            return Err(format!(
                "Record is missing the record identifier feature \
                 '{identifier_feature_name}' of feature group '{}'",
                definition.name
            ))
        }
    };

    // A record that carries no value for the group's event-time feature has no event
    // time, so neither the historic-record rule nor a TTL has anything to compute from.
    let event_time_feature_name = &definition.event_time_feature_name;
    let event_time = record
        .iter()
        .find(|feature| feature.feature_name == *event_time_feature_name)
        .and_then(|feature| feature.value_as_string.clone());

    // An event time the group identified must be a timestamp: the TTL and the
    // historic-record rule are both computed from it, so a value in neither accepted
    // format is refused rather than silently disabling both.
    let event_time_secs = match event_time.as_deref() {
        Some(at) => match parse_event_time_secs(at) {
            Some(secs) => Some(secs),
            None => {
                return Err(format!(
                    "EventTime: the event time feature '{event_time_feature_name}' must carry \
                     an ISO-8601 timestamp or seconds since the epoch, not '{at}'"
                ))
            }
        },
        None => None,
    };

    // The write's own TTL, in seconds: `ExpiresAt = EventTime + TtlDuration`, per the
    // `TtlDuration` shape's doc. The unit is a modelled enum, refused at the boundary,
    // so a value outside it only reaches here if the walk let it through.
    let record_ttl_secs = match ttl_duration {
        None => None,
        Some(ttl) => match ttl_seconds(ttl) {
            Some(secs) => Some(secs),
            None => {
                return Err(format!(
                    "TtlDuration.Unit: '{}' is not a TtlDurationUnit; expected one of {}",
                    ttl.unit,
                    crate::ops::types::TTL_DURATION_UNIT_VALUES.join(", ")
                ))
            }
        },
    };

    // The TTL the record is stored under: its own when the write named one, and otherwise
    // the default the feature group configures ([`effective_ttl_seconds`]).
    let mut stored = StoredRecord {
        identifier,
        features: record.iter().map(StoredFeatureValue::from).collect(),
        event_time,
        expires_at: None,
        expires_at_secs: None,
        ttl_secs: effective_ttl_seconds(definition, record_ttl_secs),
        soft_deleted: false,
    };
    stored.recompute_expiry();
    Ok(PreparedRecord {
        record: stored,
        event_time_secs,
    })
}

/// One feature value of a stored record.
#[derive(Debug, Clone, Default)]
pub struct StoredFeatureValue {
    pub feature_name: String,
    pub value_as_string: Option<String>,
    pub value_as_string_list: Option<Vec<String>>,
}

impl StoredFeatureValue {
    /// The stored value as the modelled `FeatureValue` the wire carries.
    pub fn to_feature_value(&self) -> crate::ops::types::FeatureValue {
        crate::ops::types::FeatureValue {
            feature_name: self.feature_name.clone(),
            value_as_string: self.value_as_string.clone(),
            value_as_string_list: self.value_as_string_list.clone(),
        }
    }
}

impl From<&crate::ops::types::FeatureValue> for StoredFeatureValue {
    fn from(value: &crate::ops::types::FeatureValue) -> Self {
        Self {
            feature_name: value.feature_name.clone(),
            value_as_string: value.value_as_string.clone(),
            value_as_string_list: value.value_as_string_list.clone(),
        }
    }
}

/// The store key for a `FeatureGroupNameOrArn`.
///
/// The member's pattern is `^(arn:aws…:feature-group/)?(<name>)$`: the ARN prefix is
/// OPTIONAL, so the ARN and the bare name denote the same feature group and must reach
/// the same store. The key is the name, which is also the key the SHARED feature-group
/// directory holds a definition under, so the ONE reading of that spelling is
/// [`::service_common::feature_store_groups::group_name`] and this delegates to it: a
/// name the data plane resolves and the store it reaches can never be two names.
pub fn group_key(name_or_arn: &str) -> &str {
    ::service_common::feature_store_groups::group_name(name_or_arn)
}

/// The ARN namespace of the resources this data plane reads (`arnNamespace` in the
/// model's `aws.api#service` trait, which is also this service's sigv4 signing name).
pub const ARN_SERVICE: &str = "sagemaker";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential scope
/// names one. It matches the region the emulator's generated shared config hands the AWS
/// CLI.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request's SigV4 credential scope names, or [`DEFAULT_REGION`] when the
/// request is unsigned.
///
/// A feature group's ARN takes its region from here, so a client signing for one region
/// is never told about a resource in another.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_owned())
}

/// `arn:aws:sagemaker:<region>:<account>:feature-group/<name>` — the ARN of the feature
/// group `name_or_arn` names, in either spelling the modelled `FeatureGroupNameOrArn`
/// admits.
///
/// This is the form the SageMaker control plane mints a `FeatureGroupArn` in (the
/// `FeatureGroupArn` shape's own pattern,
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/`), so it names the ARN
/// a group HAS — or, for a name no group was created under, the one it would have.
pub fn feature_group_arn(region: &str, name_or_arn: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("feature-group/{}", group_key(name_or_arn)),
    )
}

/// An event time in epoch seconds.
///
/// A Feature Store event-time feature is either a `String` holding an ISO-8601
/// timestamp or a `Fractional`/`Integral` holding seconds since the epoch, so both
/// forms are accepted; anything else is not a timestamp and has no instant to compare.
pub fn parse_event_time_secs(event_time: &str) -> Option<f64> {
    if let Ok(parsed) = DateTime::from_str(event_time, Format::DateTime) {
        return Some(parsed.as_secs_f64());
    }
    event_time
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|secs| secs.is_finite())
}

/// `secs` as the ISO-8601 string an `ExpiresAt` carries.
///
/// `Err` only for an instant [`DateTime`] cannot format (one outside the range a
/// timestamp can state), which no event time plus a modelled `TtlDuration` reaches.
pub fn iso8601(secs: f64) -> Result<String, aws_smithy_types::date_time::DateTimeFormatError> {
    DateTime::from_secs_f64(secs).fmt(Format::DateTime)
}

/// The TTL a record of feature group `definition` is under, in seconds: the duration the
/// write states (`record_ttl_secs`) when it states one, and otherwise the DEFAULT the
/// group's `OnlineStoreConfig.TtlDuration` configures, as the control plane recorded it.
/// `None` when neither is set, and the record then has no expiry at all.
///
/// "A record level TtlDuration is set when specifying the TtlDuration parameter using the
/// PutRecord API call. If the input TtlDuration is null or unspecified, TtlDuration is set
/// to the default feature group level TtlDuration. A record level TtlDuration supersedes
/// the group level TtlDuration" (`PutRecord`): the precedence is the same wherever a write
/// of this data plane settles a record's TTL, so it is stated once here — a whole-record
/// ingestion reads it through [`prepare_record`] and a partial update through its own
/// merge, and the two cannot disagree about which duration a record ends up under.
pub fn effective_ttl_seconds(
    definition: &::service_common::feature_store_groups::FeatureGroupRecord,
    record_ttl_secs: Option<f64>,
) -> Option<f64> {
    record_ttl_secs.or(definition.default_ttl_seconds)
}

/// `value` of the `TtlDurationUnit` `unit`, in seconds, or `None` when the unit is not
/// one the model declares.
///
/// The units are exactly the model's `TtlDurationUnit` values (see
/// `TTL_DURATION_UNIT_VALUES` in `ops::types`): Seconds, Minutes, Hours, Days, Weeks.
/// Delegated to the ONE conversion of a Feature Store `TtlDuration`, which the SageMaker
/// control plane reads a feature group's DEFAULT duration through, so a request's own
/// duration and a group's default cannot be converted two ways.
pub fn ttl_seconds(ttl: &crate::ops::types::TtlDuration) -> Option<f64> {
    ::service_common::feature_store_groups::ttl_duration_seconds(&ttl.unit, ttl.value)
}

/// The first value of an enum-typed member the model's value list does not declare.
///
/// The wire members of this service that carry an enum (`TargetStores`,
/// `DeletionMode`, `ExpirationTimeResponse`) arrive as plain strings, and a value
/// outside the enum changes what the operation would do — so it is refused with the
/// modelled `ValidationError` rather than read as "not the value I compare against".
pub fn undeclared_value<'a>(values: &'a [String], declared: &[&str]) -> Option<&'a str> {
    values
        .iter()
        .map(String::as_str)
        .find(|value| !declared.contains(value))
}

/// The refusal message for a `TargetStores` list carrying a value the model's
/// `TargetStore` enum does not declare, or `None` when every value it carries is
/// declared (an absent list always is).
///
/// Every write of this data plane reads the list the same way — "is the OnlineStore
/// among these?" ([`targets_online_store`]) — so an undeclared value would be read as
/// "not the OnlineStore" and silently turn the write into a no-op success. The boundary
/// validator refuses it first; this is the handlers' own guard, stated once so no two of
/// them word it differently.
pub fn undeclared_target_store(target_stores: Option<&Vec<String>>) -> Option<String> {
    let unknown = undeclared_value(target_stores?, crate::ops::types::TARGET_STORE_VALUES)?;
    Some(format!(
        "TargetStores: '{unknown}' is not a TargetStore; expected one of {}",
        crate::ops::types::TARGET_STORE_VALUES.join(", ")
    ))
}

/// The per-entry `ErrorCode` a batch operation of this data plane reports a refused
/// entry with: the wire code of the modelled `ValidationError` every single-record
/// operation answers the same failure with, so one classification serves the whole
/// service. It is never `ResourceNotFound`, which would say a record is missing from a
/// feature group that exists.
pub const VALIDATION_ERROR_CODE: &str = "ValidationError";

/// Whether a caller asked for `ExpiresAt` in the response.
///
/// "Parameter to request ExpiresAt in response. If Enabled, GetRecord will return the
/// value of ExpiresAt, if it is not null." A value outside the model's
/// `ExpirationTimeResponse` enum would otherwise be read as "not Enabled" and silently
/// withhold the member, so it is reported as an error for the caller to refuse with its
/// own modelled `ValidationError`. Shared by GetRecord and BatchGetRecord, which both
/// carry the member, so the two cannot drift.
pub fn expiration_time_enabled(value: Option<&str>) -> Result<bool, String> {
    match value {
        None => Ok(false),
        Some(value) if crate::ops::types::EXPIRATION_TIME_RESPONSE_VALUES.contains(&value) => {
            Ok(value == "Enabled")
        }
        Some(value) => Err(format!(
            "ExpirationTimeResponse: '{value}' is not an ExpirationTimeResponse; expected one \
             of {}",
            crate::ops::types::EXPIRATION_TIME_RESPONSE_VALUES.join(", ")
        )),
    }
}

/// Whether a `TargetStores` list includes the OnlineStore this service serves.
///
/// An absent list means every store the feature group uses — "By default, Feature Store
/// adds the record to all of the stores that you're using for the FeatureGroup."
pub fn targets_online_store(target_stores: Option<&Vec<String>>) -> bool {
    target_stores.is_none_or(|stores| stores.iter().any(|store| store == "OnlineStore"))
}

/// Whether a `TargetStores` list includes the OfflineStore a record is replicated into.
///
/// Read the same way as [`targets_online_store`], for the same sentence: an absent list is
/// every store the feature group uses, so a request that names no `TargetStores` replicates
/// into the offline store of a group that has one.
pub fn targets_offline_store(target_stores: Option<&Vec<String>>) -> bool {
    target_stores.is_none_or(|stores| stores.iter().any(|store| store == "OfflineStore"))
}

/// Wall-clock now, in epoch seconds, for the TTL comparison.
pub fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakerfeaturestoreruntimeAmazonsagemakerfeaturestoreruntimeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakerfeaturestoreruntimeAmazonsagemakerfeaturestoreruntimeState>)
}

/// The SHARED feature-group directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// The runtime API carries no feature-group definition, so the directory the SageMaker
/// control plane writes when it creates a group
/// ([`::service_common::feature_store_groups::FeatureGroupDirectory`]) is the only place
/// this service can resolve a `FeatureGroupName` against. The engine keys state by TYPE,
/// so a copy of its own inside this service's state would be invisible to the writer.
///
/// `None` only when this service is mounted on an engine that holds no state at all,
/// which is the same condition [`service_state`] answers `None` for: a handler answers it
/// with a modelled server error, never with a fresh engine.
pub fn feature_group_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::feature_store_groups::FeatureGroupDirectory>>
{
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::feature_store_groups::FeatureGroupDirectory,
            >,
        )
}

/// Why a data-plane call cannot go on: the feature group its `FeatureGroupName` names
/// does not resolve.
#[derive(Debug)]
pub enum Unresolved {
    /// No feature group of that name exists, so the REQUEST is at fault — the name
    /// denotes no feature group at all — and every operation answers it with its modelled
    /// `ValidationError` (HTTP 400), carrying this message ([`no_such_feature_group`]). It
    /// is never `ResourceNotFound`, which is reserved for a RECORD that is absent from a
    /// feature group that does resolve.
    Refused(String),
    /// This service is mounted on an engine that holds no state, so nothing can be
    /// resolved at all: a server fault, answered with the modelled `InternalFailure`.
    NoDirectory,
}

/// The definition of the feature group `name_or_arn` names, as the SageMaker control
/// plane recorded it.
///
/// This is the ONE resolution every operation of this data plane performs on its
/// `FeatureGroupName`, so no two of them can disagree about whether a name denotes a
/// feature group: a feature group exists only because `CreateFeatureGroup` created one,
/// and the runtime API has no call that creates one.
///
/// Whether the group has an `OnlineStore` is NOT decided here, because the two sides of
/// this API answer it differently and only one of them is recorded. A READ of a group
/// with no online store has nothing this service could serve, so it is refused
/// ([`no_online_store`]); a WRITE to one is accepted and replicated to the group's
/// OfflineStore where it has one ([`offline_store`]), so a write leaves the online store
/// alone and answers success — exactly as a write whose `TargetStores` names only the
/// OfflineStore does.
pub fn resolve_group<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    name_or_arn: &str,
) -> Result<::service_common::feature_store_groups::FeatureGroupRecord, Unresolved> {
    let Some(directory) = feature_group_directory(engine) else {
        return Err(Unresolved::NoDirectory);
    };
    let held = directory.lock().get(name_or_arn).cloned();
    match held {
        Some(group) => Ok(group),
        None => Err(Unresolved::Refused(no_such_feature_group(
            &feature_group_arn(region, name_or_arn),
        ))),
    }
}

/// How this data plane states a `FeatureGroupName` that denotes no feature group: the
/// group is named by `arn`, the fully qualified ARN the request's name resolves to
/// ([`feature_group_arn`]), so the refusal says which account and Region the lookup
/// looked in.
///
/// Stated ONCE, for every operation of this service: the single-record calls carry it in
/// the modelled `ValidationError` (HTTP 400) and the batch calls in the per-element
/// `ErrorMessage` beside [`VALIDATION_ERROR_CODE`], so no two of them can word one
/// absence differently. The prefix is the feature group's own — a RECORD that is absent
/// from a group that resolves is a different class, stated by the operation that looks a
/// record up.
pub fn no_such_feature_group(arn: &str) -> String {
    format!("Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name [{arn}].")
}

/// How a READ of a feature group with no `OnlineStore` is refused: there is no online
/// store for this data plane to read, so the request names nothing it can serve. Stated
/// once, so `GetRecord`, `ListRecords` and `BatchGetRecord` cannot word it three ways.
pub fn no_online_store(group: &str) -> String {
    format!("Validation Error: FeatureGroup {group} has no OnlineStore enabled.")
}
