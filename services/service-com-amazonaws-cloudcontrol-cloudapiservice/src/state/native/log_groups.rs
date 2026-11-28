// SPDX-License-Identifier: CC0-1.0

//! `AWS::Logs::LogGroup`, as it lives in the CloudWatch Logs service's own store.
//!
//! A log group is a resource of the CloudWatch Logs service: `logs:CreateLogGroup` creates
//! it, `logs:DescribeLogGroups` reports it and `logs:DeleteLogGroup` removes it, and the
//! record of it lives in that service's state in the same shared engine. So this provider
//! holds a handle to THAT store and nothing of its own: a group this emulator holds is one
//! Cloud Control API resolves, whichever API created it, and a group CloudWatch Logs no
//! longer holds is one Cloud Control API reports as not found.
//!
//! The resource model is the type's registry schema. Catalog source: the
//! `AWS::Logs::LogGroup` resource provider schema's `properties` — `LogGroupName`,
//! `Tags`, `RetentionInDays`, `LogGroupClass` and the read-only `Arn`, each carried by the
//! owning record and each applied here.
//!
//! The schema's remaining properties are ones THIS path cannot apply, so a model that asks
//! for one is refused rather than accepted and dropped — a property a caller sets has to be
//! one a later read reports:
//!
//! * `KmsKeyId`, because whether a key may encrypt a group is the KEY's policy's answer and
//!   lives in KMS, which the owning service asks on its own create
//!   (`logs::kms_key_grant::refusal_for`) and this path, applying the work the clock owes
//!   under the store's guard, cannot;
//! * `DataProtectionPolicy` and `FieldIndexPolicies`, because the owning service keeps each
//!   as a document of its own that a call of its own installs (`PutDataProtectionPolicy`,
//!   `PutIndexPolicy`) rather than as a field of the group's record.
//!
//! A model may still CARRY any of them with the value the group already holds — every read
//! reports them, so a model a client round-trips carries them — which is what lets an
//! update patch something else on a group that has one.

use ::std::collections::BTreeMap;
use ::std::time::SystemTime;

use ::serde_json::{Map, Value};
use ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state as logs;

use super::{
    epoch_millis, require_held, require_value, text, window, LogsState, NativeRefusal,
    NativeResourceProvider, UnreadableReference,
};

/// The classes of log group the owning service's model admits, which is the catalogue a
/// `LogGroupClass` outside it is refused against.
use ::service_com_amazonaws_cloudwatchlogs_logs_20140328::ops::types::LOG_GROUP_CLASS_VALUES;

/// The `TypeName` this provider stands behind.
pub const TYPE_NAME: &str = "AWS::Logs::LogGroup";

/// The property that carries the type's primary identifier, which the schema's
/// `primaryIdentifier` points at (`/properties/LogGroupName`).
const IDENTIFIER_PROPERTY: &str = "LogGroupName";

/// The type's one read-only property: the group's ARN
/// (`readOnlyProperties: ["/properties/Arn"]`).
const ARN: &str = "Arn";

/// The member of the call this type's READ handler makes that carries the reference it was
/// handed: the read reports the group through `logs:DescribeLogGroups`, which takes the name
/// as "the prefix to match" (`logGroupNamePrefix`) — a member whose shape is the owning
/// service's own `LogGroupName`, which is the same domain this type's schema publishes for
/// its `LogGroupName` property.
const READ_PREFIX_MEMBER: &str = "logGroupNamePrefix";

/// The class of the log group (`LogGroupClass`).
const LOG_GROUP_CLASS: &str = "LogGroupClass";

/// How long the group keeps its events (`RetentionInDays`).
const RETENTION_IN_DAYS: &str = "RetentionInDays";

/// The KMS key the group's data is encrypted with (`KmsKeyId`) — reported by a read, and
/// applied by nothing here (see this module's header).
const KMS_KEY_ID: &str = "KmsKeyId";

/// The data protection policy the owning service keeps on the group (`DataProtectionPolicy`),
/// installed by a call of its own and applied by nothing here.
const DATA_PROTECTION_POLICY: &str = "DataProtectionPolicy";

/// The field index policies the owning service keeps on the group (`FieldIndexPolicies`),
/// installed by a call of its own and applied by nothing here.
const FIELD_INDEX_POLICIES: &str = "FieldIndexPolicies";

/// The group's tag set (`Tags`), a list of `{ Key, Value }` pairs — read and rendered by
/// the shared reading of a tag set every provider uses.
const TAGS: &str = "Tags";

/// EVERY property this type's registry schema declares, which is the set a resource model of
/// it may carry and no more.
///
/// Catalog source: the `properties` object of the `AWS::Logs::LogGroup` resource provider
/// schema, as CloudFormation publishes it (`DescribeType`) — the same properties the model
/// reading below has an arm for, named by the constants above, so the set and the reading
/// cannot name different things. Published for the stage that judges a model's SHAPE against
/// the schema before any handler is asked ([`crate::state::registry_schema`]).
pub const DECLARED_PROPERTIES: &[&str] = &[
    ARN,
    DATA_PROTECTION_POLICY,
    FIELD_INDEX_POLICIES,
    KMS_KEY_ID,
    LOG_GROUP_CLASS,
    IDENTIFIER_PROPERTY,
    RETENTION_IN_DAYS,
    TAGS,
];

/// The CloudWatch Logs log groups of one engine, as Cloud Control API resources.
pub struct LogGroupProvider {
    /// The owning service's store. A handle, so every answer is read from the live store.
    state: ::engine::ServiceState<LogsState>,
}

impl LogGroupProvider {
    /// The provider for the CloudWatch Logs store THIS engine carries, or `None` when it
    /// carries none.
    pub fn for_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<Self> {
        logs::service_state(engine).map(|state| Self { state })
    }
}

impl NativeResourceProvider for LogGroupProvider {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn identifier_property(&self) -> &'static str {
        IDENTIFIER_PROPERTY
    }

    /// The owning service's calls this provider's handler makes: `logs:CreateLogGroup`, which
    /// creates a group, and `logs:DeleteLogGroup`, which removes one. The properties an
    /// update applies are each changed by a call of that service's own — a retention policy,
    /// a tag set, a key association — so this catalogue names none for an update and nothing
    /// about one is decided here.
    fn handler_calls(&self, operation: &str) -> &'static [&'static str] {
        // The create.
        const CREATE: &[&str] = &["logs:CreateLogGroup"];
        // The removal.
        const REMOVE: &[&str] = &["logs:DeleteLogGroup"];
        match operation {
            crate::state::OPERATION_CREATE => CREATE,
            crate::state::OPERATION_DELETE => REMOVE,
            _ => &[],
        }
    }

    /// The group's own ARN, as the OWNING service builds it from the Region and the name —
    /// the same spelling this provider reports as the group's `Arn`.
    fn resource_arn(&self, region: &str, identifier: &str) -> Option<String> {
        Some(logs::log_group_arn(region, identifier, true))
    }

    /// The read forwards the reference to `logs:DescribeLogGroups` at its
    /// [`READ_PREFIX_MEMBER`], so a reference outside what the owning service takes there —
    /// a value past either end of that shape's length or carrying a character its class
    /// excludes — is refused by that call, in that service's own words, before any group is
    /// looked for. No log group can carry such a name, so it is invalid input from the user
    /// rather than a group that is not there.
    fn unreadable_reference(&self, identifier: &str) -> Option<UnreadableReference> {
        logs::log_group_name_member_refusal(READ_PREFIX_MEMBER, identifier)
            .map(UnreadableReference::Invalid)
    }

    fn read(&self, region: &str, identifier: &str) -> Option<Value> {
        // The owning service holds its log groups in ONE namespace, so a group is the same
        // group whichever Region a call that names it was signed for.
        let _ = region;
        self.state.lock().log_groups.get(identifier).map(model)
    }

    fn list(&self, region: &str, after: Option<&str>, limit: usize) -> Vec<(String, Value)> {
        let _ = region;
        // The owning store keys its groups by name, so its own ordered range IS the
        // window a listing pages over — nothing is sorted, and nothing outside the
        // window is read.
        let store = self.state.lock();
        window(&store.log_groups, after, limit)
            .map(|(name, group)| (name.clone(), model(group)))
            .collect()
    }

    fn create(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        // The name a group is created under is the OWNING service's to accept: the domain
        // its model gives `logGroupName` (the shared reading of it,
        // `::service_common::cloudwatch_logs`) and the prefix that service keeps for
        // groups of its own — so nothing is created here under a name that service's own
        // API would refuse.
        if !::service_common::cloudwatch_logs::is_log_group_name(identifier) {
            return Err(NativeRefusal::Invalid(format!(
                "{IDENTIFIER_PROPERTY} {}",
                ::service_common::cloudwatch_logs::log_group_name_requirement()
            )));
        }
        if ::service_common::cloudwatch_logs::is_reserved_log_group_name(identifier) {
            return Err(NativeRefusal::Rejected(
                ::service_common::cloudwatch_logs::reserved_log_group_name_message(),
            ));
        }
        let desired = DesiredModel::read(
            identifier,
            properties,
            &Held {
                arn: &logs::log_group_arn(region, identifier, true),
                // No group holds a class or a key yet: this create is what settles the
                // class, and a key is one this path cannot apply at all.
                class: None,
                kms_key_id: None,
            },
        )?;
        let mut store = self.state.lock();
        // The group is created in the owning service's store under its own rules: a name
        // it already holds is that service's own already-exists verdict, and its quota
        // bounds what this create may add.
        if store.log_groups.contains_key(identifier) {
            return Err(NativeRefusal::AlreadyExists);
        }
        if store.log_groups.len() >= logs::MAX_LOG_GROUPS {
            return Err(NativeRefusal::AtCapacity(logs::log_groups_quota_message()));
        }
        let mut group = logs::LogGroupRecord::new(region, identifier, epoch_millis(at));
        desired.apply(&mut group);
        let created = model(&group);
        store.log_groups.insert(identifier.to_string(), group);
        Ok(created)
    }

    fn update(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        // One namespace per account, and a group's record carries no instant a model
        // changes.
        let _ = (region, at);
        let mut store = self.state.lock();
        let held = store
            .log_groups
            .get(identifier)
            .ok_or(NativeRefusal::NotFound)?;
        let (arn, class, kms_key_id) = (
            held.arn.clone(),
            held.log_group_class.clone(),
            held.kms_key_id.clone(),
        );
        let desired = DesiredModel::read(
            identifier,
            properties,
            &Held {
                arn: &arn,
                class: Some(&class),
                kms_key_id: kms_key_id.as_deref(),
            },
        )?;
        let group = store
            .log_groups
            .get_mut(identifier)
            .ok_or(NativeRefusal::NotFound)?;
        desired.apply(group);
        Ok(model(group))
    }

    fn delete(&self, region: &str, identifier: &str, at: SystemTime) -> Result<(), NativeRefusal> {
        let _ = (region, at);
        let mut store = self.state.lock();
        let group = store
            .log_groups
            .get(identifier)
            .ok_or(NativeRefusal::NotFound)?;
        // Deletion protection "blocks all deletion operations until it is explicitly
        // disabled", so the owning service raises its own error about the group as it is —
        // a verdict the handler maps to none of its own — and the resource stays.
        if group.deletion_protection_enabled {
            return Err(NativeRefusal::Rejected(
                logs::log_group_delete_protected_message(),
            ));
        }
        store.log_groups.remove(identifier);
        Ok(())
    }
}

/// The resource model one log group renders as — the schema's properties the owning record
/// carries, and only those it HAS: a group under no retention, no key and no tags carries
/// none of those properties, the same way the owning service's own reader reports none.
fn model(group: &logs::LogGroupRecord) -> Value {
    let mut model = Map::new();
    model.insert(IDENTIFIER_PROPERTY.to_string(), Value::from(group.name.clone()));
    model.insert(ARN.to_string(), Value::from(group.arn.clone()));
    model.insert(
        LOG_GROUP_CLASS.to_string(),
        Value::from(group.log_group_class.clone()),
    );
    if let Some(days) = group.retention_in_days {
        model.insert(RETENTION_IN_DAYS.to_string(), Value::from(days));
    }
    if let Some(key) = &group.kms_key_id {
        model.insert(KMS_KEY_ID.to_string(), Value::from(key.clone()));
    }
    if !group.tags.is_empty() {
        model.insert(TAGS.to_string(), super::tag_list(group.tags.iter()));
    }
    Value::Object(model)
}

/// What the owning service already HOLDS for a group a model is read against: the values
/// the properties this path does not apply may be carried with, and nothing else.
struct Held<'a> {
    /// The group's own ARN, which every read reports.
    arn: &'a str,
    /// The class the group is of — `None` where no group holds one yet, which is the create
    /// that settles it.
    class: Option<&'a str>,
    /// The key the group's data is encrypted with, if any.
    kms_key_id: Option<&'a str>,
}

/// What a resource model asks a log group to be: the properties this path APPLIES, having
/// been checked against the owning service's own rules.
struct DesiredModel {
    /// The class the group is to be of, or `None` to leave the record's own.
    log_group_class: Option<String>,
    /// The retention the group is to be under — `None` for a model that names none, which
    /// is a group that keeps its events forever.
    retention_in_days: Option<i32>,
    /// The tag set the group is to carry.
    tags: BTreeMap<String, String>,
}

impl DesiredModel {
    /// Read `properties` as the model of the group `identifier`, which the owning service
    /// holds `held` for — or state why that service would not take it.
    ///
    /// The identity, the read-only ARN and every property this path does not apply may be
    /// CARRIED (a read reports them, so a model a client round-trips carries them) but only
    /// with the value the group already holds: a value accepted here and never applied
    /// would be one no later read reports.
    fn read(
        identifier: &str,
        properties: &Value,
        held: &Held<'_>,
    ) -> Result<Self, NativeRefusal> {
        let Some(model) = properties.as_object() else {
            return Err(NativeRefusal::Invalid(format!(
                "{TYPE_NAME} requires a resource model of its properties"
            )));
        };
        let mut desired = Self {
            log_group_class: None,
            retention_in_days: None,
            tags: BTreeMap::new(),
        };
        for (property, value) in model {
            match property.as_str() {
                IDENTIFIER_PROPERTY => require_value(property, value, identifier)?,
                ARN => require_value(property, value, held.arn)?,
                // Whether this key may encrypt this group is the key policy's answer, which
                // lives in KMS; see this module's header.
                KMS_KEY_ID => require_held(TYPE_NAME, property, value, held.kms_key_id)?,
                LOG_GROUP_CLASS => match held.class {
                    // The class is settled when the group is created: the owning service
                    // publishes no operation that moves a group from one class to another.
                    Some(held) => require_value(property, value, held)?,
                    None => {
                        let class = text(property, value)?;
                        if !LOG_GROUP_CLASS_VALUES.contains(&class.as_str()) {
                            return Err(NativeRefusal::Invalid(logs::invalid_value_message(
                                "logGroupClass",
                                LOG_GROUP_CLASS_VALUES.iter().copied(),
                            )));
                        }
                        desired.log_group_class = Some(class);
                    }
                },
                RETENTION_IN_DAYS => {
                    let days = value
                        .as_i64()
                        .and_then(|days| i32::try_from(days).ok())
                        .filter(|days| logs::RETENTION_IN_DAYS_VALUES.contains(days))
                        .ok_or_else(|| {
                            NativeRefusal::Invalid(logs::invalid_value_message(
                                "retention",
                                logs::RETENTION_IN_DAYS_VALUES,
                            ))
                        })?;
                    desired.retention_in_days = Some(days);
                }
                TAGS => desired.tags = tag_set(value)?,
                // Properties of the type the OWNING service installs with a call of its
                // own, which this path does not make; see this module's header.
                other if DECLARED_PROPERTIES.contains(&other) => {
                    return Err(unsupported(other));
                }
                other => {
                    return Err(NativeRefusal::Invalid(format!(
                        "{other} is not a property of {TYPE_NAME}"
                    )));
                }
            }
        }
        Ok(desired)
    }

    /// Write this model onto `group`. A property the model does not carry is CLEARED, so
    /// the record holds exactly the model that was applied rather than a mixture of it and
    /// whatever the group was before.
    fn apply(self, group: &mut logs::LogGroupRecord) {
        if let Some(class) = self.log_group_class {
            group.log_group_class = class;
        }
        group.retention_in_days = self.retention_in_days;
        group.tags = self.tags;
    }
}

/// The refusal for a property of the type's schema that THIS path does not apply — the
/// shared wording, so a caller can tell it from a property the type does not declare at
/// all.
fn unsupported(property: &str) -> NativeRefusal {
    super::unsupported(TYPE_NAME, property)
}

/// The tag set `value` carries: the shared reading of a tag set, bounded by the owning
/// service's own per-resource tag quota.
fn tag_set(value: &Value) -> Result<BTreeMap<String, String>, NativeRefusal> {
    // The quota is on what the LIST carries, judged before any two entries of one key are
    // folded together, so a list past the quota is refused however many keys it repeats.
    let entries = super::tag_entries(value)?;
    if entries.len() > logs::MAX_TAGS_PER_RESOURCE {
        return Err(NativeRefusal::Rejected(logs::too_many_tags_message()));
    }
    let tags: BTreeMap<String, String> = entries.into_iter().collect();
    // The tag-key namespace the owning service keeps for Amazon Web Services' own tags is
    // not a caller's to assign, in its own words: a model naming one is refused here for
    // the same reason that service's own tagging operations refuse it, so nothing writes a
    // tag set into its store that its own API would not take.
    if logs::names_reserved_tag_key(tags.keys().map(String::as_str)) {
        return Err(NativeRefusal::Rejected(
            logs::RESERVED_TAG_KEY_MESSAGE.to_string(),
        ));
    }
    Ok(tags)
}
