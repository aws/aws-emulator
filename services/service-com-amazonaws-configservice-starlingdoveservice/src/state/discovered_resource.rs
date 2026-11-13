// SPDX-License-Identifier: CC0-1.0

//! One stored discovered resource: the resource Config's recorder captured and the
//! configuration items it recorded for it, with the projections the configuration-item
//! read paths report them through.

use super::*;

/// One resource Config has discovered, as its configuration recorder captured it.
///
/// A discovered resource SURVIVES its own deletion in Config's inventory —
/// `ListDiscoveredResources` can be asked to include deleted resources, and reports
/// `resourceDeletionTime` for each one — so a deletion is recorded as the instant the
/// resource went away rather than by dropping the entry.
#[derive(Debug, Clone, Default)]
pub struct DiscoveredResourceEntry {
    /// The resource's type, a `com.amazonaws.configservice#ResourceType` value.
    pub resource_type: String,
    /// The id of the resource (`ResourceIdentifier$resourceId`).
    pub resource_id: String,
    /// The resource's custom name, when it has one
    /// (`ResourceIdentifier$resourceName`: "The custom name of the resource (if
    /// available)").
    pub resource_name: Option<String>,
    /// When set, the instant the resource was deleted
    /// (`ResourceIdentifier$resourceDeletionTime`: "The time that the resource was
    /// deleted").
    pub deleted_at: Option<SystemTime>,
    /// The ARN the OWNING service reported for this resource, for an observation that
    /// carried one (`ConfigurationItem$arn`: "Amazon Resource Name (ARN) associated with
    /// the resource").
    ///
    /// A recorder that reads a resource from its own service's API is told that resource's
    /// ARN by the service that mints it, and an ARN a service stated is the one the
    /// configuration item reports; `None` for a resource nothing handed one over, whose
    /// ARN is DERIVED from the type and the id that identify it ([`recorded_resource_arn`])
    /// instead.
    pub arn: Option<String>,
    /// The `SchemaVersionId` the resource's configuration item was recorded under,
    /// for a resource whose configuration state `PutResourceConfig` recorded.
    pub schema_version_id: Option<String>,
    /// The `Configuration` document as it was recorded, byte for byte.
    pub configuration: Option<String>,
    /// The `supplementaryConfiguration` blocks recorded with the configuration item:
    /// "a string to string map that contains additional contents for the resource
    /// configuration", which Config "returns for certain resource types to supplement
    /// the information returned for the `configuration` field". Each value is a JSON
    /// document, as that member's own documentation states ("needs to be deserialized
    /// using `json.loads()`").
    pub supplementary_configuration: BTreeMap<String, String>,
    /// The tags the configuration item was recorded with
    /// (`PutResourceConfigRequest$Tags`).
    pub tags: BTreeMap<String, String>,
    /// When Config first recorded a configuration item for this resource, and when it
    /// captured the configuration item that is its CURRENT one — the instant that item
    /// reports as its `configurationItemCaptureTime`. Both are `None` for a resource no
    /// configuration item has been recorded for.
    ///
    /// Neither instant moves while the resource stays in the state the current item
    /// records: observing that same state again records no configuration item, so there
    /// is no new capture instant to report
    /// ([`ComAmazonawsConfigserviceStarlingdoveserviceState::record_discovered_resource`]).
    /// A state the resource LEAVES keeps the instant its own item was captured at in
    /// [`Self::history`].
    pub first_recorded_at: Option<SystemTime>,
    pub last_recorded_at: Option<SystemTime>,
    /// The configuration items recorded for this resource BEFORE the current one,
    /// oldest first: one per state the resource has left. Written by
    /// [`ComAmazonawsConfigserviceStarlingdoveserviceState::record_discovered_resource`],
    /// which keeps at most [`CONFIGURATION_ITEMS_KEPT`] of them.
    pub history: Vec<RecordedConfigurationState>,
}

impl DiscoveredResourceEntry {
    /// The modelled `ResourceIdentifier`, read back out of state.
    pub fn to_identifier(&self) -> ResourceIdentifier {
        ResourceIdentifier {
            resource_type: Some(self.resource_type.clone()),
            resource_id: Some(self.resource_id.clone()),
            resource_name: self.resource_name.clone(),
            resource_deletion_time: self.deleted_at.map(timestamp),
        }
    }

    /// The modelled `AggregateResourceIdentifier`, read back out of state: the same
    /// resource as [`Self::to_identifier`] names, plus the SOURCE this store's data
    /// comes from.
    ///
    /// An aggregate read reports a resource as the account and Region it was recorded
    /// in — "the resource type, ID, (if available) the custom resource name, the source
    /// account, and source region" — and this store stands for ONE account in ONE
    /// Region, so `source_account_id` and `source_region` are that account and that
    /// Region rather than anything a caller sent.
    pub fn to_aggregate_identifier(
        &self,
        account_id: &str,
        region: &str,
    ) -> AggregateResourceIdentifier {
        AggregateResourceIdentifier {
            source_account_id: account_id.to_string(),
            source_region: region.to_string(),
            resource_id: self.resource_id.clone(),
            resource_type: self.resource_type.clone(),
            resource_name: self.resource_name.clone(),
        }
    }

    /// The resource's CURRENT configuration item, as the configuration-item read paths
    /// report it: this account's data in `aws_region`, recorded at
    /// `recording_frequency` (see [`ComAmazonawsConfigserviceStarlingdoveserviceState::recording_frequency_of`]).
    ///
    /// A `BaseConfigurationItem` is the configuration item WITHOUT the resource's tags
    /// and relationships (`BatchGetAggregateResourceConfig`: "The API does not return
    /// tags and relationships"), so the tags the inventory holds are deliberately not
    /// reported here, while `supplementaryConfiguration` — the extra configuration
    /// blocks a recorder captures beside the configuration itself — carries the ones
    /// recorded for this resource, and is absent when none was.
    ///
    /// The resource's own `arn` is the ARN of the resource this item records — the one
    /// the owning service reported for it where the observation carried one
    /// ([`Self::arn`]), else the one derived from the type and the id that identify it in
    /// the Region the item is reported under ([`recorded_resource_arn`]) — and it is the
    /// only member of the item that carries the resource's ARN, since `resourceId` and
    /// `resourceName` carry the bare id and name.
    ///
    /// Two members stay unset because nothing in this emulator knows them:
    /// `availabilityZone` belongs to the service that owns the resource, and
    /// `resourceCreationTime` is the instant that service created it, neither of which
    /// a recorded configuration item carries (`PutResourceConfig` records the type, the
    /// id, the name, the schema version and the configuration document, and nothing
    /// else).
    pub fn to_base_configuration_item(
        &self,
        aws_region: &str,
        recording_frequency: &str,
    ) -> BaseConfigurationItem {
        // Daily recording delivers a configuration item separately from capturing it,
        // which is the only frequency that reports either member
        // (`BaseConfigurationItem$configurationItemDeliveryTime`: "if you are using
        // daily recording, this field will be populated. However, if you are using
        // continuous recording, this field will be omitted since the delivery time is
        // instantaneous as the CI is available right away").
        let daily = recording_frequency == RECORDING_FREQUENCY_DAILY;
        BaseConfigurationItem {
            // "The version number of the resource configuration" — the schema version
            // the configuration document was recorded under
            // (`PutResourceConfigRequest$SchemaVersionId`).
            version: self.schema_version_id.clone(),
            account_id: Some(::emulator_server::emulator_account_id().to_string()),
            configuration_item_capture_time: self.last_recorded_at.map(timestamp),
            configuration_item_status: Some(self.configuration_item_status().to_string()),
            configuration_state_id: self.configuration_state_id(),
            // The ARN the owning service reported, where the observation carried one;
            // otherwise the one the type and the id derive.
            arn: self
                .arn
                .clone()
                .or_else(|| recorded_resource_arn(&self.resource_type, &self.resource_id, aws_region)),
            resource_type: Some(self.resource_type.clone()),
            resource_id: Some(self.resource_id.clone()),
            resource_name: self.resource_name.clone(),
            aws_region: Some(aws_region.to_string()),
            availability_zone: None,
            resource_creation_time: None,
            configuration: self.configuration.clone(),
            supplementary_configuration: (!self.supplementary_configuration.is_empty()).then(
                || {
                    self.supplementary_configuration
                        .iter()
                        .map(|(block, value)| (block.clone(), value.clone()))
                        .collect()
                },
            ),
            recording_frequency: daily.then(|| recording_frequency.to_string()),
            configuration_item_delivery_time: daily
                .then(|| self.last_recorded_at.map(timestamp))
                .flatten(),
        }
    }

    /// The resource's CURRENT configuration item as the FULL `ConfigurationItem` the
    /// single-resource aggregate read reports, which is
    /// [`Self::to_base_configuration_item`] plus the four members a
    /// `BaseConfigurationItem` leaves out.
    ///
    /// Built from the base item so the two read paths cannot drift: every member they
    /// share is computed in ONE place.
    ///
    /// Of those four extra members, the resource's TAGS are reported — the tags the
    /// inventory recorded with the configuration item, which only the APIs that
    /// document dropping them (`BatchGetAggregateResourceConfig`: "The API does not
    /// return tags and relationships") leave out — while `relationships` and
    /// `relatedEvents` stay unset because nothing in this emulator knows them: a
    /// relationship is another resource the owning service links this one to, and a
    /// related event is a CloudTrail event id, neither of which a recorded
    /// configuration item carries.
    pub fn to_configuration_item(
        &self,
        aws_region: &str,
        recording_frequency: &str,
    ) -> ConfigurationItem {
        let base = self.to_base_configuration_item(aws_region, recording_frequency);
        ConfigurationItem {
            version: base.version,
            account_id: base.account_id,
            configuration_item_capture_time: base.configuration_item_capture_time,
            configuration_item_status: base.configuration_item_status,
            configuration_state_id: base.configuration_state_id,
            // "Unique MD5 hash that represents the configuration item's state", which a
            // caller uses "to compare the states of two or more configuration items" of
            // one resource — so it is the digest of the state this item records
            // ([`Self::configuration_item_md5_hash`]).
            configuration_item_md5_hash: Some(self.configuration_item_md5_hash()),
            arn: base.arn,
            resource_type: base.resource_type,
            resource_id: base.resource_id,
            resource_name: base.resource_name,
            aws_region: base.aws_region,
            availability_zone: base.availability_zone,
            resource_creation_time: base.resource_creation_time,
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
            related_events: None,
            relationships: None,
            configuration: base.configuration,
            supplementary_configuration: base.supplementary_configuration,
            recording_frequency: base.recording_frequency,
            configuration_item_delivery_time: base.configuration_item_delivery_time,
        }
    }

    /// The `ConfigurationItemStatus` of the resource's current configuration item: the
    /// resource has been deleted, it was newly discovered, or its configuration has been
    /// updated.
    ///
    /// The member's own documentation states the three: "OK – The resource configuration
    /// has been updated", "ResourceDiscovered – The resource was newly discovered" and
    /// "ResourceDeleted – The resource was deleted". So the status follows from WHICH item
    /// of the resource this is: the FIRST one Config recorded — the one that has no state
    /// before it in [`Self::history`] — is the discovery of the resource, and every later
    /// one records a configuration that has been updated since. A past item read back out
    /// of the history is projected with the states before IT
    /// ([`Self::configuration_items`]), so each item keeps the status it was recorded
    /// with.
    pub fn configuration_item_status(&self) -> &'static str {
        if self.deleted_at.is_some() {
            CONFIGURATION_ITEM_STATUS_RESOURCE_DELETED
        } else if self.history.is_empty() {
            CONFIGURATION_ITEM_STATUS_RESOURCE_DISCOVERED
        } else {
            CONFIGURATION_ITEM_STATUS_OK
        }
    }

    /// The `configurationItemMD5Hash` of the configuration item this entry reports:
    /// "unique MD5 hash that represents the configuration item's state", which a caller
    /// uses "to compare the states of two or more configuration items that are associated
    /// with the same resource".
    ///
    /// It is therefore the digest of the STATE the item records — the resource the item is
    /// about, the status it was recorded with, the configuration document, the
    /// supplementary blocks recorded beside it and the tags — and of nothing else: two
    /// items that record the same state of one resource carry the same hash, every read of
    /// ONE recorded state reports the same hash, and a state the resource changed into
    /// carries a different one. That is what lets a caller compare two items and tell
    /// whether the resource changed between them.
    ///
    /// The state is digested as one JSON document, so a value that could run into the next
    /// cannot make two different states digest alike.
    pub fn configuration_item_md5_hash(&self) -> String {
        use ::md5::Digest;

        let state = ::serde_json::json!({
            "resourceType": self.resource_type,
            "resourceId": self.resource_id,
            "resourceName": self.resource_name,
            "configurationItemStatus": self.configuration_item_status(),
            "configuration": self.configuration,
            "supplementaryConfiguration": self
                .supplementary_configuration
                .iter()
                .map(|(block, value)| (block.clone(), value.clone()))
                .collect::<::std::collections::BTreeMap<String, String>>(),
            "tags": self
                .tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<::std::collections::BTreeMap<String, String>>(),
        });
        let mut digest = ::md5::Md5::new();
        digest.update(state.to_string().as_bytes());
        format!("{:x}", digest.finalize())
    }

    /// The `configurationStateId` of the current configuration item — "an identifier
    /// that indicates the ordering of the configuration items of a resource" — which
    /// is therefore DERIVED from the instant the item was captured, in milliseconds,
    /// so each state the resource goes through carries a later id than the one before
    /// it and every read of ONE recorded state reports the same id. A resource no
    /// configuration item has been recorded for has no state to order, so it carries
    /// none.
    pub fn configuration_state_id(&self) -> Option<String> {
        self.last_recorded_at.map(|at| {
            at.duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .to_string()
        })
    }

    /// EVERY configuration item recorded for this resource, newest first: the current
    /// one ([`Self::to_configuration_item`]) and then each state the resource has left
    /// ([`Self::history`]), most recent first.
    ///
    /// This is the order `GetResourceConfigHistory` reports by default ("By default, the
    /// results are listed in reverse chronological order",
    /// `GetResourceConfigHistoryRequest$chronologicalOrder`); a request asking for
    /// `Forward` reverses it.
    ///
    /// A past item differs from the current one only in what a later item CHANGED — the
    /// configuration document, its supplementary blocks, the tags and the instant it was
    /// captured — so each is projected off this entry with those members replaced, which
    /// keeps one statement of how a configuration item is rendered. Each past item carries
    /// the states recorded BEFORE it, so the status it reports is the one it was recorded
    /// with ([`Self::configuration_item_status`]): the oldest item of the resource is its
    /// discovery.
    pub fn configuration_items(
        &self,
        aws_region: &str,
        recording_frequency: &str,
    ) -> Vec<ConfigurationItem> {
        let mut items = vec![self.to_configuration_item(aws_region, recording_frequency)];
        for (index, state) in self.history.iter().enumerate().rev() {
            let past = Self {
                configuration: state.configuration.clone(),
                supplementary_configuration: state.supplementary_configuration.clone(),
                tags: state.tags.clone(),
                last_recorded_at: state.recorded_at,
                // An earlier item is one the resource was still there for: the deletion
                // belongs to the item that recorded it, which is the current one.
                deleted_at: None,
                history: self.history[..index].to_vec(),
                ..self.clone()
            };
            items.push(past.to_configuration_item(aws_region, recording_frequency));
        }
        items
    }

    /// The state this entry's CURRENT configuration item records, as the history keeps
    /// it once a later item replaces it.
    pub(super) fn recorded_state(&self) -> RecordedConfigurationState {
        RecordedConfigurationState {
            configuration: self.configuration.clone(),
            supplementary_configuration: self.supplementary_configuration.clone(),
            tags: self.tags.clone(),
            recorded_at: self.last_recorded_at,
        }
    }

    /// Whether `other` records a DIFFERENT state of this resource than this entry's
    /// current item does: a new configuration item is recorded for a state that changed,
    /// while observing the state that is already recorded records nothing at all.
    pub(super) fn records_another_state(&self, other: &Self) -> bool {
        self.configuration != other.configuration
            || self.supplementary_configuration != other.supplementary_configuration
            || self.tags != other.tags
            || self.deleted_at.is_some() != other.deleted_at.is_some()
    }
}
