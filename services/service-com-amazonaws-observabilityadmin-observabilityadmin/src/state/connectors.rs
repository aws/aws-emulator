// SPDX-License-Identifier: CC0-1.0

//! The CONNECTOR half of this service's state: a connector to a third-party cloud
//! provider, and the telemetry manifest that connector carries.
//!
//! A connector is created in the region the request was signed for and the service
//! mints its id, because `ConnectorIdentifier` accepts "the ID of the connector" or its
//! ARN and never its name — so a record is keyed by `(region, id)`, its ARN is indexed
//! onto that key, and the caller-supplied `Name`, which "must be unique in the account
//! and Region (case-sensitive)", has an index of its own that a second create of the
//! same name is refused on.
//!
//! The telemetry manifest is NOT a resource of its own: it belongs to exactly one
//! connector, `GetConnectorTelemetryManifest` answers the modelled 404 while the
//! connector carries none, and `DeleteConnector` "deletes a connector and its telemetry
//! manifest" — so it is held ON the connector record rather than in a map beside it,
//! and the version `PutConnectorTelemetryManifest` answers is that record's own
//! counter.
//!
//! Nothing here reaches into the third-party provider: the emulator holds no Azure
//! tenant, so a connector's own fields (its configuration, its role, its id, its
//! manifest and that manifest's version) are the whole of what this module knows, and
//! `ListResourcesByConnector` reports the inventory this emulator has DISCOVERED in the
//! provider, which is none.

use std::collections::BTreeMap;

use super::*;
use crate::ops::types::{Connector, ConnectorConfiguration, ConnectorSummary, TelemetryManifest};

/// The resource portion a connector's ARN carries before its provider and id, so an
/// ARN reads
/// `arn:aws:observabilityadmin:<region>:<account>:connector/<provider>/<id>`.
///
/// The provider segment is part of the form `ConnectorIdentifier`'s own pattern admits
/// (`connector/(azure/)?[a-z0-9]+`), written in lower case, so the ARN this service
/// mints is one that pattern accepts back.
pub const CONNECTOR_ARN_RESOURCE_PREFIX: &str = "connector/";

/// The `ResourceType` a modelled error names the connector RESOURCE by — its own API
/// name, for the reason [`RULE_RESOURCE_TYPE`] states.
pub const CONNECTOR_RESOURCE_TYPE: &str = "Connector";

/// The `ResourceType` a duplicate-name `ConflictException` names a connector by: the
/// INPUT MEMBER whose value collided (`CreateConnector$Name`), for the reason
/// [`RULE_NAME_RESOURCE_TYPE`] states.
pub const CONNECTOR_NAME_RESOURCE_TYPE: &str = "Name";

/// The `ResourceType` a modelled error names a connector's TELEMETRY MANIFEST by. It
/// is a different resource from the connector: a read of a connector that carries no
/// manifest is a 404 about the manifest, not about the connector.
pub const CONNECTOR_MANIFEST_RESOURCE_TYPE: &str = "TelemetryManifest";

/// The `ResourceNotFoundException` message for a request naming a connector no record
/// answers: fixed text, with no part of the request in it, in the form this service's
/// other read refusals take ([`PIPELINE_NOT_FOUND_MESSAGE`]).
pub const CONNECTOR_NOT_FOUND_MESSAGE: &str = "Connector not found";

/// The `ResourceNotFoundException` message for a read of the telemetry manifest of a
/// connector that carries none, which `GetConnectorTelemetryManifest` documents as the
/// answer in that case. Fixed text.
pub const CONNECTOR_MANIFEST_NOT_FOUND_MESSAGE: &str =
    "The connector has no telemetry manifest";

/// The `ConflictException` message for a create whose connector name is already in use
/// in the region the request was signed for. Fixed text that names the INPUT MEMBER
/// whose value collided, not the value itself, the way
/// [`PIPELINE_NAME_ALREADY_EXISTS_MESSAGE`] does.
pub const CONNECTOR_NAME_ALREADY_EXISTS_MESSAGE: &str = "The specified Name already exists";

/// The `ValidationException` message for a request that reuses a `ClientToken` with
/// different parameters, which both `CreateConnector` and
/// `PutConnectorTelemetryManifest` document as a `ValidationException`. Fixed text.
pub const CLIENT_TOKEN_REUSED_MESSAGE: &str =
    "The ClientToken was already used with different request parameters";

/// The `ConflictException` message for a `PutConnectorTelemetryManifest` whose
/// `ExpectedVersion` is not the manifest's current version.
///
/// Its variable parts are the two versions — the one the request expected and the one
/// the store holds — because the member's own documentation is what the caller must
/// act on: "if the value doesn't match the current version, the request fails with a
/// ConflictException", and the current version is what it must pass next.
pub fn manifest_version_conflict_message(expected: i64, current: i64) -> String {
    format!(
        "The telemetry manifest version is {current}, not the expected {expected}"
    )
}

/// The one value `ConnectorProvider` carries (`CONNECTOR_PROVIDER_VALUES`), which is
/// the provider of an Azure connector configuration.
pub const CONNECTOR_PROVIDER_AZURE: &str = "AZURE";

/// The most connector records this emulator will hold ACROSS every region.
///
/// Emulator resource guard, NOT an AWS quota: this service publishes no connector
/// quota. It bounds the one growth site a request can drive, and it is store-wide for
/// the reason [`MAX_STORED_PIPELINE_RECORDS`] states — a record is keyed by the region
/// of the caller's own credential scope, so a per-region cap alone would hand a caller
/// a fresh budget for every scope it signs with. Each record also holds a telemetry
/// manifest the model bounds, so this bounds the stored bytes too. A create past it is
/// refused with the modelled `ServiceQuotaExceededException` before a record is
/// allocated.
pub const MAX_STORED_CONNECTOR_RECORDS: usize = 500;

/// A connector record's store key: `(region, id)`.
pub type ConnectorKey = (String, String);

/// The telemetry manifest one connector carries, with the version a caller passes back
/// as `ExpectedVersion`.
#[derive(Debug, Clone)]
pub struct StoredTelemetryManifest {
    /// The manifest body exactly as the caller supplied it, behind an `Arc` for the
    /// reason the telemetry rule's body is: it is caller-sized, and a replace must not
    /// cost a second copy of it while a reader still holds the old one.
    pub manifest: ::std::sync::Arc<TelemetryManifest>,
    /// `Version`: 1 for the first manifest the connector was given, one more on every
    /// replace, so the value a read reports is the one the next `ExpectedVersion` must
    /// carry.
    pub version: i64,
    /// `LastUpdateTimeStamp`, in epoch milliseconds.
    pub last_update_timestamp_ms: i64,
    /// The `ClientToken` of the request that wrote this manifest, and the fingerprint
    /// of that request's manifest, so a retry of the SAME token "returns the stored
    /// version without an error" and the same token with a different manifest is
    /// refused.
    pub client_token: Option<String>,
    /// The fingerprint of the manifest the request carrying [`Self::client_token`]
    /// wrote ([`manifest_fingerprint`]).
    pub fingerprint: String,
}

/// One stored connector to a third-party cloud provider.
#[derive(Debug, Clone)]
pub struct StoredConnector {
    /// The id this service assigned the connector, which its ARN carries and which
    /// `ConnectorIdentifier` accepts on its own.
    pub id: String,
    /// `Name`, the caller-supplied name, unique per region.
    pub name: String,
    pub region: String,
    /// `arn:aws:observabilityadmin:<region>:<account>:connector/<provider>/<id>`.
    pub arn: String,
    /// `Provider`, one of `ConnectorProvider`'s own values, derived from which
    /// configuration the create carried.
    pub provider: String,
    /// The configuration body exactly as the caller supplied it, stored verbatim so a
    /// read reports back what was written.
    pub configuration: ::std::sync::Arc<ConnectorConfiguration>,
    pub role_arn: String,
    /// `CreatedTimeStamp`, in epoch milliseconds.
    pub created_timestamp_ms: i64,
    pub tags: BTreeMap<String, String>,
    /// The `ClientToken` the create carried, with the fingerprint of that create's
    /// parameters, so a retry of the SAME token answers this record and the same token
    /// with different parameters is refused.
    pub client_token: Option<String>,
    /// The fingerprint of the create that carried [`Self::client_token`]
    /// ([`connector_fingerprint`]).
    pub fingerprint: String,
    /// The telemetry manifest this connector carries, `None` until a
    /// `PutConnectorTelemetryManifest` gives it one.
    pub manifest: Option<StoredTelemetryManifest>,
}

impl StoredConnector {
    /// This record's store key.
    pub fn key(&self) -> ConnectorKey {
        (self.region.clone(), self.id.clone())
    }

    /// This record as the `Connector` a create or a read reports.
    pub fn connector(&self) -> Connector {
        Connector {
            arn: self.arn.clone(),
            provider: self.provider.clone(),
            connector_configuration: (*self.configuration).clone(),
            role_arn: self.role_arn.clone(),
            name: Some(self.name.clone()),
            created_time_stamp: Some(self.created_timestamp_ms),
        }
    }

    /// This record as the summary `ListConnectors` reports, read from the same record
    /// a `GetConnector` of this connector reports, so a listing and a read never
    /// disagree. The shape declares no configuration or role: `GetConnector` is where
    /// those are read.
    pub fn summary(&self) -> ConnectorSummary {
        ConnectorSummary {
            arn: self.arn.clone(),
            provider: self.provider.clone(),
            name: Some(self.name.clone()),
            created_time_stamp: Some(self.created_timestamp_ms),
        }
    }

    /// The version of the manifest this connector carries, and 0 when it carries none
    /// — the version an `ExpectedVersion` must name for the FIRST manifest, which
    /// `PutConnectorTelemetryManifest` documents as the one a caller omits.
    pub fn manifest_version(&self) -> i64 {
        self.manifest.as_ref().map_or(0, |held| held.version)
    }
}

/// The ARN of the connector `id` of `provider` in `region`.
///
/// The provider segment is lower case, which is the form `ConnectorIdentifier`'s own
/// pattern admits, so a caller can hand this ARN straight back to `GetConnector`.
pub fn connector_arn(region: &str, provider: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{CONNECTOR_ARN_RESOURCE_PREFIX}{}/{id}",
            provider.to_lowercase()
        ),
    )
}

/// A newly minted connector id.
///
/// `ConnectorIdentifier` admits a bare id of `[a-z0-9]+`, which [`new_resource_id`]'s
/// lowercase hexadecimal token is, so the id this service mints is one its own
/// identifier constraint accepts back.
pub fn new_connector_id() -> String {
    new_resource_id()
}

/// The `Provider` a connector configuration names, one of `ConnectorProvider`'s own
/// values: the union's variant IS the provider, so there is nothing to validate here
/// and nothing a request could name that the deserializer would admit.
pub fn configuration_provider(configuration: &ConnectorConfiguration) -> &'static str {
    match configuration {
        ConnectorConfiguration::Azure(_) => CONNECTOR_PROVIDER_AZURE,
    }
}

/// The fingerprint of a `CreateConnector`'s parameters, which a repeated `ClientToken`
/// is compared on: the same token with the same parameters "returns the connector from
/// the first request without an error", and with different parameters — "including
/// different tags" — is a `ValidationException`.
///
/// It is the request's own members rendered through their derived `Debug`, so a member
/// added to the modelled input shape is part of the comparison without this function
/// being touched. The tag map is folded into a `BTreeMap` first, because a `HashMap`'s
/// `Debug` order is not stable and two renderings of one map could otherwise differ.
pub fn connector_fingerprint(
    name: &str,
    role_arn: &str,
    configuration: &ConnectorConfiguration,
    tags: Option<&::std::collections::HashMap<String, String>>,
) -> String {
    let tags: BTreeMap<&str, &str> = tags
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect()
        })
        .unwrap_or_default();
    format!("{name:?}|{role_arn:?}|{configuration:?}|{tags:?}")
}

/// The fingerprint of a telemetry manifest, which a repeated `ClientToken` on
/// `PutConnectorTelemetryManifest` is compared on: the same token with the same
/// manifest "returns the stored version without an error", and with a different
/// manifest is a `ValidationException`.
///
/// Rendered through the manifest's derived `Debug` for the reason
/// [`connector_fingerprint`] states; the manifest shape holds no map, so the rendering
/// is stable.
pub fn manifest_fingerprint(manifest: &TelemetryManifest) -> String {
    format!("{manifest:?}")
}

impl ComAmazonawsObservabilityadminObservabilityadminState {
    /// The key of the connector `identifier` names for a caller signed for `region`,
    /// accepting both forms `ConnectorIdentifier`'s documentation allows: "the Amazon
    /// Resource Name (ARN) of the connector" or "the ID of the connector".
    ///
    /// An ARN must match a stored ARN in EVERY field and name a connector in the
    /// caller's own region, so an ARN for another partition, region or account
    /// resolves to nothing rather than to the connector that shares its id.
    pub fn resolve_connector_key(&self, region: &str, identifier: &str) -> Option<ConnectorKey> {
        if identifier.starts_with("arn:") {
            return self
                .connector_arn_index
                .get(identifier)
                .filter(|(connector_region, _)| connector_region == region)
                .cloned();
        }
        let key = (region.to_string(), identifier.to_string());
        self.connectors.contains_key(&key).then_some(key)
    }

    /// The connector `identifier` names in `region`.
    pub fn connector(&self, region: &str, identifier: &str) -> Option<&StoredConnector> {
        let key = self.resolve_connector_key(region, identifier)?;
        self.connectors.get(&key)
    }

    /// The connector a create's `ClientToken` was already answered with, when that
    /// token named one in `region`.
    pub fn connector_for_client_token(
        &self,
        region: &str,
        token: &str,
    ) -> Option<&StoredConnector> {
        self.connectors
            .range((region.to_string(), String::new())..)
            .take_while(|((connector_region, _), _)| connector_region == region)
            .map(|(_, record)| record)
            .find(|record| record.client_token.as_deref() == Some(token))
    }

    /// Whether a connector in `region` already carries `name`, which
    /// `CreateConnector$Name` requires to be unique there.
    pub fn connector_name_taken(&self, region: &str, name: &str) -> bool {
        self.connector_name_index
            .contains_key(&(region.to_string(), name.to_string()))
    }

    /// How many connector records the store holds in total, across every region,
    /// which is what [`MAX_STORED_CONNECTOR_RECORDS`] bounds.
    pub fn connector_record_count(&self) -> usize {
        self.connectors.len()
    }

    /// Whether storing `records` more connectors would take the store past
    /// [`MAX_STORED_CONNECTOR_RECORDS`]. A write path asks this BEFORE it allocates,
    /// so the refusal costs nothing.
    pub fn would_exceed_connector_store(&self, records: usize) -> bool {
        self.connector_record_count().saturating_add(records) > MAX_STORED_CONNECTOR_RECORDS
    }

    /// Store a connector record and index it by ARN and by name.
    ///
    /// The key is `(region, id)` and the id is freshly minted per create
    /// ([`new_connector_id`]), so an insert never lands on an occupied key; the name
    /// index is what the create established was free first
    /// ([`Self::connector_name_taken`]).
    pub fn insert_connector(&mut self, record: StoredConnector) {
        self.connector_arn_index
            .insert(record.arn.clone(), record.key());
        self.connector_name_index
            .insert((record.region.clone(), record.name.clone()), record.id.clone());
        self.connectors.insert(record.key(), record);
    }

    /// Replace the telemetry manifest of the connector at `key`, answering the version
    /// the connector carries after the write.
    ///
    /// The version is one more than the version it carried, so the value a read reports
    /// is the one the next `ExpectedVersion` must carry. A key no record answers
    /// answers `None`: every caller resolves the record under this same lock first.
    pub fn put_connector_manifest(
        &mut self,
        key: &ConnectorKey,
        manifest: TelemetryManifest,
        client_token: Option<String>,
        now_ms: i64,
    ) -> Option<i64> {
        let record = self.connectors.get_mut(key)?;
        let version = record.manifest_version().saturating_add(1);
        record.manifest = Some(StoredTelemetryManifest {
            fingerprint: manifest_fingerprint(&manifest),
            manifest: ::std::sync::Arc::new(manifest),
            version,
            last_update_timestamp_ms: now_ms,
            client_token,
        });
        Some(version)
    }

    /// Remove the connector `identifier` names in `region`, with its telemetry
    /// manifest — "deletes a connector and its telemetry manifest" — answering the
    /// record that was removed, or `None` when the identifier names none.
    ///
    /// Every handle a caller holds on the connector goes with it: the record, its ARN
    /// index entry and its name index entry, so the name is free for a later create and
    /// neither handle resolves past the instant the record stops existing.
    pub fn remove_connector(
        &mut self,
        region: &str,
        identifier: &str,
    ) -> Option<StoredConnector> {
        let key = self.resolve_connector_key(region, identifier)?;
        let record = self.connectors.remove(&key)?;
        self.connector_arn_index.remove(&record.arn);
        self.connector_name_index
            .remove(&(record.region.clone(), record.name.clone()));
        Some(record)
    }
}
