// SPDX-License-Identifier: CC0-1.0

//! One stored connector to a third-party cloud service provider: the connector as it
//! was created, the projections the connector read paths report it through, and the
//! identifiers, ARNs and refusal texts those paths are built from.

use super::*;

/// Microsoft Azure, one of the third-party cloud service providers a connector
/// connects to (`com.amazonaws.configservice#Provider`: "`AZURE` – The connector is
/// for Microsoft Azure").
pub const PROVIDER_AZURE: &str = "AZURE";

/// Google Cloud, the other published `Provider` ("`GCP` – The connector is for Google
/// Cloud").
///
/// Which provider a connector connects to is what decides WHICH provider-specific
/// members its reads report ([`ConnectorEntry::to_summary`]), because each of them is
/// an identifier of one provider's own.
pub const PROVIDER_GCP: &str = "GCP";

/// The only filter `ListConnectors` publishes
/// (`com.amazonaws.configservice#ConnectorFilterName`; `ConnectorFilter$filterName`:
/// "Currently, only provider is supported").
pub const CONNECTOR_FILTER_PROVIDER: &str = "provider";

/// How many connectors one `ListConnectors` page carries when the request does not
/// ask for a page size — `MaxResults` defaults to 0, which is "unspecified" rather
/// than "no results". 100 is the `ConnectorSummaries` length maximum (0..100), and
/// also `ListConnectorsMaxResults`'s own maximum, so it is the largest page the model
/// lets one response carry.
pub const LIST_CONNECTORS_PAGE_SIZE: usize = 100;

/// The most connectors this store can hold: the bound past which a connector could
/// not be reported any more, which is what `MaxNumberOfConnectorsExceededException`
/// stands for.
///
/// NOTHING ENFORCES IT TODAY, because nothing in `src/` inserts a connector:
/// `PutConnector` verifies a provider configuration's credentials WITH the provider
/// before it materializes anything, and verifying them is an outbound connection this
/// emulator does not make, so every configuration is refused (see
/// [`UNVERIFIABLE_AZURE_CREDENTIALS_MESSAGE`] and `crate::ops::put_connector`). It is
/// kept as the documented cap of the connector store — the number the read paths' page
/// size is taken from — and is where the quota check belongs if a connector ever
/// enters the store again.
///
/// The model carries no number for this quota, so the limit is CHOSEN: it is the
/// `ConnectorSummaries` length maximum (0..100), the most connectors the model can
/// report in one page. Config documents the connector quota per account per Region
/// and one engine's store stands for ONE account in ONE Region, so — like
/// [`CONFIG_RULES_QUOTA`] and [`CONFIGURATION_AGGREGATORS_QUOTA`] — it is enforced
/// over the WHOLE store: the Region a request is scoped to is the caller's own, so
/// counting per Region would let a caller keep one bucket of connectors per Region
/// string it invents. This is also what bounds the work `ListConnectors` does to
/// filter one Region's connectors out of the store.
pub const CONNECTORS_QUOTA: usize = 100;

/// One persisted connector to a third-party cloud service provider.
#[derive(Debug, Clone)]
pub struct ConnectorEntry {
    /// The connector's name. `PutConnector` takes no name, so Config derives it from
    /// the configuration the connector was created with ([`connector_name`]).
    pub name: String,
    pub arn: String,
    /// The Region this connector belongs to — the Region of the request that created
    /// it, and the Region segment of [`Self::arn`]. A connector is a REGIONAL
    /// resource, so every operation serves only the connectors of the Region its
    /// request is scoped to.
    pub region: String,
    /// The `Provider` this connector's configuration names.
    pub provider: String,
    /// The provider-specific configuration the connector was created with. Connectors
    /// cannot be updated (`PutConnector`: "To update the connector configuration, you
    /// must delete all associated configuration recorders, delete the connector, and
    /// recreate it"), so this never changes once stored.
    pub connector_configuration: ConnectorConfiguration,
    pub created_at: SystemTime,
    /// The tags the connector was CREATED with (`PutConnector`: "Tags are added at
    /// creation and cannot be updated with this operation"), and whatever the tagging
    /// operations have since made of them: they are the one tag set of this resource,
    /// reached through
    /// [`ComAmazonawsConfigserviceStarlingdoveserviceState::tags_of`].
    pub tags: TagSet,
}

impl ConnectorEntry {
    /// The modelled `Connector`, read back out of state.
    pub fn to_shape(&self) -> Connector {
        Connector {
            name: self.name.clone(),
            arn: self.arn.clone(),
            connector_configuration: self.connector_configuration.clone(),
            created_time: timestamp(self.created_at),
        }
    }

    /// The modelled `ConnectorSummary` a listing reports for this connector.
    ///
    /// Every member beside the connector's own identity is an identifier of ONE
    /// provider's own, and a summary reports it for exactly the connectors connected to
    /// that provider: `tenantIdentifier` is "the Azure tenant identifier for the
    /// connector. Returned only for Azure connectors", and `gcpOrganizationId` and
    /// `serviceAccountEmail` are "returned only for Google Cloud connectors". So each
    /// is read off the provider configuration the connector was CREATED with, and a
    /// connector of the other provider carries none of them — rather than an empty
    /// value standing in for an identifier that provider has no concept of.
    ///
    /// `gcpOrganizationId` is optional within the Google Cloud configuration itself
    /// ("required if the management project belongs to an organization ... omit it only
    /// if the management project doesn't belong to an organization"), so a Google Cloud
    /// connector created without one reports none either.
    pub fn to_summary(&self) -> ConnectorSummary {
        let azure = self.connector_configuration.azure.as_ref();
        let gcp = self.connector_configuration.gcp.as_ref();
        ConnectorSummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            provider: self.provider.clone(),
            tenant_identifier: azure.map(|azure| azure.tenant_identifier.clone()),
            gcp_organization_id: gcp.and_then(|gcp| gcp.gcp_organization_id.clone()),
            service_account_email: gcp.map(|gcp| gcp.service_account_email.clone()),
            created_time: timestamp(self.created_at),
        }
    }
}

/// The ARN resource type of a connector, as Config spells it in the ARNs it mints.
pub const CONNECTOR_ARN_RESOURCE_TYPE: &str = "connector";

/// The `ConnectorName` length maximum (`com.amazonaws.configservice#ConnectorName`,
/// length 1..256), which is also the length of the resource portion of a connector's
/// ARN, because that portion is exactly the connector's name.
pub const CONNECTOR_NAME_MAX_LEN: usize = 256;

/// The ARN of a connector: `arn:aws:config:<region>:<account>:connector/<name>`.
pub fn connector_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{CONNECTOR_ARN_RESOURCE_TYPE}/{name}"),
    )
}

/// The name of the connector a configuration describes: the provider it connects to
/// and the identifier the configuration names within that provider (the Azure tenant,
/// the Google Cloud organization).
///
/// `PutConnector` carries no name, so the name is DERIVED from the configuration —
/// which is also why one provider configuration can have only one connector: a second
/// connector for it would need the same name.
pub fn connector_name(provider: &str, identifier: &str) -> String {
    format!("{}-{identifier}", provider.to_lowercase())
}

/// The group lengths of the canonical, hyphenated form of a GUID — `8-4-4-4-12`
/// hexadecimal digits — which is the form Microsoft Entra ID (Azure AD) publishes a
/// directory (tenant) identifier and an application (client) identifier in. It is the
/// same canonical form a uuid this service MINTS is written in ([`UUID_GROUPS`]), so
/// the widths are named once.
pub const AZURE_IDENTIFIER_GUID_GROUPS: [usize; 5] = UUID_GROUPS;

/// Whether `value` has the form an Azure tenant or client identifier takes: a GUID in
/// its canonical hyphenated form.
///
/// `AzureTenantIdentifier` and `AzureClientIdentifier` carry no `@pattern` — the model
/// constrains only their length (1..128) — so the FORM is checked here.
///
/// This is a FORM check, not a parse: what matters is the spelling the provider
/// publishes, and a UUID parser (the `uuid` crate's `try_parse`) accepts the simple
/// 32-character, braced and URN spellings too, so it would still need this group
/// check around it. Checking the five groups' lengths and their hexadecimal digits is
/// how the framework itself asserts this form (`vendor/server/src/lib.rs`,
/// `vendor/smithy-schema-server/src/query.rs`), and it keeps the check free of a
/// dependency the rest of this crate does not need.
pub fn is_azure_guid(value: &str) -> bool {
    let mut groups = value.split('-');
    AZURE_IDENTIFIER_GUID_GROUPS.iter().all(|length| {
        groups.next().is_some_and(|group| {
            group.len() == *length && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    }) && groups.next().is_none()
}

/// How Config words a `ConnectorConfiguration` that names no provider at all.
///
/// `ConnectorConfiguration` carries one member per provider and its doc states "You
/// must specify exactly one provider configuration", so a configuration carrying none
/// names nothing to connect to. The text is fixed — it is built from no part of the
/// request — because it is the only thing that tells this refusal apart from the other
/// `ValidationException` legs of `PutConnector`.
pub const CONNECTOR_CONFIGURATION_NO_PROVIDER_MESSAGE: &str =
    "ConnectorConfiguration must contain at least one provider";

/// How Config words a `ConnectorConfiguration` that names SEVERAL providers.
///
/// The other half of "you must specify exactly one provider configuration": a
/// configuration naming two providers describes no single connection to make, and a
/// connector connects to one provider — the one its `Provider` reports and whose
/// identifiers its reads carry. Fixed, for the same reason as
/// [`CONNECTOR_CONFIGURATION_NO_PROVIDER_MESSAGE`].
pub const CONNECTOR_CONFIGURATION_SEVERAL_PROVIDERS_MESSAGE: &str =
    "ConnectorConfiguration must contain exactly one provider";

/// How Config words a tenant identifier whose form is not an Azure one
/// ([`is_azure_guid`]). Fixed, and it withholds the value the caller sent.
pub const INVALID_AZURE_TENANT_IDENTIFIER_MESSAGE: &str = "Invalid tenant identifier";

/// How Config words a client identifier whose form is not an Azure one
/// ([`is_azure_guid`]). Fixed, for the same reason as
/// [`INVALID_AZURE_TENANT_IDENTIFIER_MESSAGE`].
pub const INVALID_AZURE_CLIENT_IDENTIFIER_MESSAGE: &str = "Invalid client identifier";

/// How Config words a provider credential it could not verify with the provider.
///
/// A connector IS the credential Config connects to the third-party cloud service
/// provider with, so the tenant/client pair is verified with that provider before any
/// connector is materialized, and a pair that does not verify is refused. Verifying
/// one means calling the provider's identity service — an outbound connection this
/// emulator does not make — so no pair presented here can be verified and every
/// well-formed configuration is refused with this text rather than answered with a
/// connector no credential stands behind.
pub const UNVERIFIABLE_AZURE_CREDENTIALS_MESSAGE: &str =
    "Unable to verify Azure credentials for the provided tenant configuration.";

/// How Config words a Google Cloud credential it could not verify with the provider.
///
/// The Google Cloud leg of [`UNVERIFIABLE_AZURE_CREDENTIALS_MESSAGE`], and refused for
/// the same reason: the credential a Google Cloud connector IS — the service account
/// Config accesses the project's resources as, through the workload identity pool
/// provider that trusts Config — is verified WITH Google Cloud before any connector is
/// materialized, and verifying it means calling Google's identity service, an outbound
/// connection this emulator does not make. The text names the credential this
/// provider's configuration carries, because it is what tells this refusal apart from
/// the Azure one.
pub const UNVERIFIABLE_GCP_CREDENTIALS_MESSAGE: &str =
    "Unable to verify Google Cloud credentials for the provided service account \
     configuration.";

/// The Region and the connector name an `Arn` names, or `None` when the value is not
/// a well-formed connector ARN of this service.
///
/// A well-formed ARN of another partition, Region or account parses here and simply
/// names no connector this store holds — "not found" rather than "not valid".
pub fn connector_arn_parts(arn: &str) -> Option<(&str, &str)> {
    let mut segments = arn.splitn(6, ':');
    if segments.next()? != "arn" {
        return None;
    }
    let partition = segments.next()?;
    let service = segments.next()?;
    let region = segments.next()?;
    let account = segments.next()?;
    let resource = segments.next()?;
    // A connector is a regional resource, so its ARN names a Region.
    if partition.is_empty() || service != ARN_SERVICE || region.is_empty() || account.is_empty() {
        return None;
    }
    let name = resource
        .strip_prefix(CONNECTOR_ARN_RESOURCE_TYPE)?
        .strip_prefix('/')?;
    // `ConnectorName` is 1..256 characters, and the resource portion of a connector
    // ARN is exactly that name.
    if name.is_empty() || name.len() > CONNECTOR_NAME_MAX_LEN || name.contains('/') {
        return None;
    }
    Some((region, name))
}

/// Why `arn` is not a well-formed connector ARN. BUILT FROM THE REQUEST: it quotes
/// the value the caller sent and names the form the member takes.
///
/// Reached from `PutThirdPartyServiceLinkedConfigurationRecorder` alone, and that is
/// deliberate: there the `ConnectorArn` is a reference to a dependency the recorder
/// needs, so the refusal says which value could not name one. The operations whose
/// `Arn` IS the connector they act on answer the single
/// [`CONNECTOR_ARN_NOT_VALID_MESSAGE`] instead. A well-formed ARN that names no
/// connector is not this refusal: there the recorder the service principal names
/// cannot be resolved at all, which is
/// [`UNRECOGNIZED_RECORDER_SERVICE_PRINCIPAL_MESSAGE`].
pub fn invalid_connector_arn_message(arn: &str) -> String {
    format!(
        "The Arn '{arn}' provided in the request is not a valid connector ARN. Please \
         provide an ARN of the form \
         arn:aws:{ARN_SERVICE}:<region>:<account>:{CONNECTOR_ARN_RESOURCE_TYPE}/<name>."
    )
}

/// The ONE text an operation whose `Arn` names the connector it acts on refuses that
/// `Arn` with when it does not resolve to a connector of this account in the Region
/// the request is scoped to.
///
/// Resolving the `Arn` is all `GetConnector` and `DeleteConnector` do, and a value
/// that is no connector ARN and a well-formed one that names no connector both fail to
/// resolve, so both are refused with this one text — fixed, carrying no part of the
/// request — and neither says which of the two it was: a caller cannot learn from a
/// refusal whether a connector by that ARN exists.
pub const CONNECTOR_ARN_NOT_VALID_MESSAGE: &str = "The connector ARN provided is not valid.";
