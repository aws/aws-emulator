// SPDX-License-Identifier: CC0-1.0

//! The THIRD-PARTY TELEMETRY ENRICHMENT half of this service's state.
//!
//! An enrichment "adds resource metadata, such as tags, from a third-party telemetry
//! source to the telemetry that CloudWatch receives from the resources of that
//! source". It is started with a name that "must be unique in your account and Region"
//! and that "you can use instead of the ARN to identify the enrichment in other
//! operations" — so a record is keyed by `(region, Name)` and its ARN is indexed onto
//! that key, the way the telemetry rule's is.
//!
//! The enrichment is the one resource here with a non-terminal status of its own:
//! `Start` leaves it `Initializing` ("CloudWatch is setting up the enrichment") and it
//! reports `Running` once the setup the start began comes due. That instant is stored
//! on the record and every read derives the status from it and the clock, so no
//! operation sleeps and no read has to write. A `Stop` is immediate — the modelled
//! status set carries no stopping value — and it leaves the record in place reporting
//! `Stopped`, which is the value `ListThirdPartyTelemetryEnrichments$Status` filters
//! on.
//!
//! The source configuration names an Amazon Web Services Config connector by ARN. That
//! connector is AWS Config's resource, not this service's, so the ARN is stored
//! verbatim and reported back and nothing here claims it exists; for the same reason
//! `ServiceLinkedRecorderArn` — "the service-linked configuration recorder that
//! CloudWatch created in your account" — is NOT reported: this change creates no
//! recorder in AWS Config, so reporting one would claim a resource that service does
//! not hold.

use std::collections::BTreeMap;

use super::*;
use crate::ops::types::{
    AzureTelemetrySourceConfigurationOutput, TelemetrySourceConfigurationInput,
    TelemetrySourceConfigurationOutput, ThirdPartyTelemetryEnrichmentSummary,
};

/// The resource portion an enrichment's ARN carries before its id, so an ARN reads
/// `arn:aws:observabilityadmin:<region>:<account>:telemetry-enrichments/<id>` — the
/// form `ThirdPartyTelemetryEnrichmentArn`'s own pattern fixes.
pub const ENRICHMENT_ARN_RESOURCE_PREFIX: &str = "telemetry-enrichments/";

/// The leading character an enrichment id carries.
///
/// `ThirdPartyTelemetryEnrichmentArn`'s pattern ends `telemetry-enrichments/[a-z][a-z0-9]{9,63}`,
/// so the id starts with a LETTER where [`new_resource_id`]'s hexadecimal token may
/// start with a digit: this is the letter prefixed to it, and it is the resource's own
/// initial.
pub const ENRICHMENT_ID_PREFIX: char = 'e';

/// The `ResourceType` a modelled error names the enrichment RESOURCE by — its own API
/// name, for the reason [`RULE_RESOURCE_TYPE`] states.
pub const ENRICHMENT_RESOURCE_TYPE: &str = "TelemetryEnrichment";

/// The `ResourceType` a duplicate-name `ConflictException` names an enrichment by: the
/// INPUT MEMBER whose value collided (`StartThirdPartyTelemetryEnrichment$Name`), for
/// the reason [`RULE_NAME_RESOURCE_TYPE`] states.
pub const ENRICHMENT_NAME_RESOURCE_TYPE: &str = "Name";

/// The `ResourceNotFoundException` message for a request naming an enrichment no
/// record answers: fixed text, with no part of the request in it.
pub const ENRICHMENT_NOT_FOUND_MESSAGE: &str = "Telemetry enrichment not found";

/// The `ConflictException` message for a start whose enrichment name is already in use
/// in the region the request was signed for. Fixed text naming the INPUT MEMBER whose
/// value collided, not the value itself.
pub const ENRICHMENT_NAME_ALREADY_EXISTS_MESSAGE: &str = "The specified Name already exists";

/// The `ConflictException` message for a start whose connector is already enriching:
/// "you can use each connector for only one enrichment". Fixed text.
pub const ENRICHMENT_CONNECTOR_IN_USE_MESSAGE: &str =
    "The specified connector is already used by another telemetry enrichment";

/// The `ConflictException` message for a stop of an enrichment that is already
/// stopped. Fixed text.
pub const ENRICHMENT_ALREADY_STOPPED_MESSAGE: &str = "The telemetry enrichment is already stopped";

/// `ThirdPartyTelemetryEnrichmentStatus` of an enrichment whose setup has not come due
/// yet: "CloudWatch is setting up the enrichment".
pub const THIRD_PARTY_ENRICHMENT_STATUS_INITIALIZING: &str = "Initializing";

/// `ThirdPartyTelemetryEnrichmentStatus` of an enrichment whose setup has come due:
/// "the enrichment is active".
pub const THIRD_PARTY_ENRICHMENT_STATUS_RUNNING: &str = "Running";

/// `ThirdPartyTelemetryEnrichmentStatus` of an enrichment a stop has stopped.
pub const THIRD_PARTY_ENRICHMENT_STATUS_STOPPED: &str = "Stopped";

/// The one value `ThirdPartyTelemetrySource` carries
/// (`THIRD_PARTY_TELEMETRY_SOURCE_VALUES`), which is the source of an Azure source
/// configuration.
pub const THIRD_PARTY_TELEMETRY_SOURCE_AZURE: &str = "Azure";

/// How long after a `StartThirdPartyTelemetryEnrichment` the enrichment's
/// `Initializing` -> `Running` transition comes due.
///
/// A CHOSEN span, not an AWS-documented duration: the modelled status set says a
/// started enrichment is `Initializing` while "CloudWatch is setting up the
/// enrichment" and `Running` once it is active, and this is the span this emulator
/// takes to get there. It is deliberately non-zero, so a caller that reads straight
/// after the start still observes `Initializing`.
pub const ENRICHMENT_ACTIVATION_DELAY_MS: i64 = 3_000;

/// The most third-party telemetry enrichment records this emulator will hold ACROSS
/// every region.
///
/// Emulator resource guard, NOT an AWS quota: this service publishes no enrichment
/// quota. It is store-wide for the reason [`MAX_STORED_CONNECTOR_RECORDS`] states. A
/// start past it is refused with the modelled `ServiceQuotaExceededException` before a
/// record is allocated.
pub const MAX_STORED_ENRICHMENT_RECORDS: usize = 500;

/// An enrichment record's store key: `(region, Name)`.
pub type EnrichmentKey = (String, String);

/// One stored third-party telemetry enrichment.
#[derive(Debug, Clone)]
pub struct StoredThirdPartyEnrichment {
    /// `Name`, the caller-supplied name, unique per region and one of the two handles
    /// an identifier takes.
    pub name: String,
    pub region: String,
    /// `arn:aws:observabilityadmin:<region>:<account>:telemetry-enrichments/<id>`.
    pub arn: String,
    /// `TelemetrySource`, one of `ThirdPartyTelemetrySource`'s own values, derived
    /// from which source configuration the start carried.
    pub telemetry_source: String,
    /// `AwsConfigConnectorArn`, stored exactly as the caller supplied it. It names
    /// AWS Config's resource, which this service does not own and does not resolve.
    pub aws_config_connector_arn: String,
    /// Epoch-millisecond instant the start was accepted.
    pub created_timestamp_ms: i64,
    /// The instant the setup the start began comes due, from when on the enrichment
    /// reports `Running`.
    pub running_at_ms: i64,
    /// Whether a `Stop` has stopped this enrichment. A stop is immediate — the
    /// modelled status set carries no stopping value — so this is a flag and not a
    /// due instant.
    pub stopped: bool,
    pub tags: BTreeMap<String, String>,
}

impl StoredThirdPartyEnrichment {
    /// This record's store key.
    pub fn key(&self) -> EnrichmentKey {
        (self.region.clone(), self.name.clone())
    }

    /// The `Status` this enrichment reports at `now_ms`: `Stopped` once a stop has
    /// stopped it, otherwise `Initializing` until the setup the start began comes due
    /// and `Running` from then on.
    pub fn status_at(&self, now_ms: i64) -> &'static str {
        if self.stopped {
            THIRD_PARTY_ENRICHMENT_STATUS_STOPPED
        } else if now_ms < self.running_at_ms {
            THIRD_PARTY_ENRICHMENT_STATUS_INITIALIZING
        } else {
            THIRD_PARTY_ENRICHMENT_STATUS_RUNNING
        }
    }

    /// `StatusReason`, which "is present when the status is `Impaired`".
    ///
    /// Nothing in this emulator impairs an enrichment — there is no third-party
    /// environment here for a setup to fail against — so no status it reports has a
    /// reason, and the member is absent rather than carrying a sentence about a
    /// healthy enrichment.
    pub fn status_reason(&self) -> Option<String> {
        None
    }

    /// The `TelemetrySourceConfigurationOutput` a read reports: the connector ARN the
    /// start carried, under the variant of the source it names.
    ///
    /// `ServiceLinkedRecorderArn` is deliberately absent: it is "the service-linked
    /// configuration recorder that CloudWatch created in your account", a resource of
    /// AWS Config, and this service creates none there — so reporting an ARN for one
    /// would name a recorder that service does not hold.
    pub fn source_configuration(&self) -> TelemetrySourceConfigurationOutput {
        TelemetrySourceConfigurationOutput::Azure(AzureTelemetrySourceConfigurationOutput {
            aws_config_connector_arn: Some(self.aws_config_connector_arn.clone()),
            service_linked_recorder_arn: None,
        })
    }

    /// The tags this enrichment carries, as the modelled `TagMapOutput` a read
    /// reports: always a map, EMPTY when nothing tagged it, for the reason
    /// [`StoredTelemetryPipeline::tag_map`] states.
    pub fn tag_map(&self) -> ::std::collections::HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// This record as the summary a listing reports, as it stands at `now_ms`, read
    /// from the same record a `GetThirdPartyTelemetryEnrichment` reports, so a listing
    /// and a read never disagree.
    pub fn summary(&self, now_ms: i64) -> ThirdPartyTelemetryEnrichmentSummary {
        ThirdPartyTelemetryEnrichmentSummary {
            name: Some(self.name.clone()),
            telemetry_source: Some(self.telemetry_source.clone()),
            status: Some(self.status_at(now_ms).to_string()),
        }
    }
}

/// The ARN of the enrichment `id` in `region`.
pub fn third_party_enrichment_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ENRICHMENT_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// A newly minted enrichment id.
///
/// `ThirdPartyTelemetryEnrichmentArn`'s pattern requires a leading letter, so the id
/// is [`ENRICHMENT_ID_PREFIX`] followed by [`new_resource_id`]'s token — the one form
/// this service's own handles take — which keeps the whole id inside the pattern's
/// `[a-z][a-z0-9]{9,63}`.
pub fn new_third_party_enrichment_id() -> String {
    format!("{ENRICHMENT_ID_PREFIX}{}", new_resource_id())
}

/// The `TelemetrySource` a source configuration names, one of
/// `ThirdPartyTelemetrySource`'s own values: the union's variant IS the source, so
/// there is nothing to validate here.
pub fn source_configuration_source(configuration: &TelemetrySourceConfigurationInput) -> &'static str {
    match configuration {
        TelemetrySourceConfigurationInput::Azure(_) => THIRD_PARTY_TELEMETRY_SOURCE_AZURE,
    }
}

/// The AWS Config connector ARN a source configuration names.
pub fn source_configuration_connector_arn(
    configuration: &TelemetrySourceConfigurationInput,
) -> &str {
    match configuration {
        TelemetrySourceConfigurationInput::Azure(azure) => &azure.aws_config_connector_arn,
    }
}

impl ComAmazonawsObservabilityadminObservabilityadminState {
    /// The key of the enrichment `identifier` names for a caller signed for `region`,
    /// accepting both forms the member's documentation allows: "the name of the
    /// enrichment" or "the Amazon Resource Name (ARN) of the enrichment".
    ///
    /// An ARN must match a stored ARN in EVERY field and name an enrichment in the
    /// caller's own region, so an ARN for another partition, region or account
    /// resolves to nothing rather than to the enrichment that shares its name.
    pub fn resolve_third_party_enrichment_key(
        &self,
        region: &str,
        identifier: &str,
    ) -> Option<EnrichmentKey> {
        if identifier.starts_with("arn:") {
            return self
                .third_party_enrichment_arn_index
                .get(identifier)
                .filter(|(enrichment_region, _)| enrichment_region == region)
                .cloned();
        }
        let key = (region.to_string(), identifier.to_string());
        self.third_party_enrichments.contains_key(&key).then_some(key)
    }

    /// The enrichment `identifier` names in `region`.
    pub fn third_party_enrichment(
        &self,
        region: &str,
        identifier: &str,
    ) -> Option<&StoredThirdPartyEnrichment> {
        let key = self.resolve_third_party_enrichment_key(region, identifier)?;
        self.third_party_enrichments.get(&key)
    }

    /// Whether an enrichment in `region` already carries `name`, which
    /// `StartThirdPartyTelemetryEnrichment$Name` requires to be unique there.
    pub fn third_party_enrichment_name_taken(&self, region: &str, name: &str) -> bool {
        self.third_party_enrichments
            .contains_key(&(region.to_string(), name.to_string()))
    }

    /// Whether an enrichment in `region` already enriches from `connector_arn`:
    /// "you can use each connector for only one enrichment".
    ///
    /// A STOPPED enrichment still holds its connector: the record is still there, the
    /// name is still taken and a stop is not a delete, so the connector it names is
    /// still the one that enrichment uses.
    pub fn third_party_enrichment_connector_taken(
        &self,
        region: &str,
        connector_arn: &str,
    ) -> bool {
        self.third_party_enrichments
            .range((region.to_string(), String::new())..)
            .take_while(|((enrichment_region, _), _)| enrichment_region == region)
            .any(|(_, record)| record.aws_config_connector_arn == connector_arn)
    }

    /// How many enrichment records the store holds in total, across every region,
    /// which is what [`MAX_STORED_ENRICHMENT_RECORDS`] bounds.
    pub fn third_party_enrichment_record_count(&self) -> usize {
        self.third_party_enrichments.len()
    }

    /// Whether storing `records` more enrichments would take the store past
    /// [`MAX_STORED_ENRICHMENT_RECORDS`]. A write path asks this BEFORE it allocates,
    /// so the refusal costs nothing.
    pub fn would_exceed_third_party_enrichment_store(&self, records: usize) -> bool {
        self.third_party_enrichment_record_count()
            .saturating_add(records)
            > MAX_STORED_ENRICHMENT_RECORDS
    }

    /// Store an enrichment record and index it by ARN.
    ///
    /// The key is `(region, Name)`, so a caller must have established that the name is
    /// free in that region first ([`Self::third_party_enrichment_name_taken`]) —
    /// every write path here does — because inserting over an occupied key would
    /// replace that record.
    pub fn insert_third_party_enrichment(&mut self, record: StoredThirdPartyEnrichment) {
        self.third_party_enrichment_arn_index
            .insert(record.arn.clone(), record.key());
        self.third_party_enrichments.insert(record.key(), record);
    }

    /// Stop the enrichment at `key`, answering the record as it stands after the stop,
    /// or `None` when no record answers that key.
    ///
    /// The record STAYS: a stop "stops adding resource metadata", it does not retire
    /// the enrichment — `ListThirdPartyTelemetryEnrichments` filters on the `Stopped`
    /// status, so a stopped enrichment is still one of the account's.
    pub fn stop_third_party_enrichment(
        &mut self,
        key: &EnrichmentKey,
    ) -> Option<&StoredThirdPartyEnrichment> {
        let record = self.third_party_enrichments.get_mut(key)?;
        record.stopped = true;
        Some(record)
    }
}
