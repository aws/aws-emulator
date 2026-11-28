// SPDX-License-Identifier: CC0-1.0

//! The cloud connectors this service holds: the connection between Systems Manager and
//! a third-party cloud environment.
//!
//! `CreateCloudConnector` mints one, `UpdateCloudConnector` changes the members a
//! caller may change and `GetCloudConnector` reports it. All three reach the one store
//! below ([`super::ComAmazonawsSsmAmazonssmState::cloud_connectors`]), so whatever a
//! write persisted is what a later read reports.
//!
//! A connector is reached through the endpoint of ONE Region — Systems Manager is a
//! regional service and the `CloudConnectorArn` the read reports carries that Region —
//! so the store is keyed by `(region, connector id)`. The id is the server-generated
//! `CloudConnectorId` (`len 36..36`, pattern
//! `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), minted by
//! [`new_cloud_connector_id`]: a caller never chooses it, so a create is the only thing
//! that admits a key here and an id no create minted names no connector
//! ([`CLOUD_CONNECTOR_NOT_FOUND`]).
//!
//! Everything a read reports is what the create persisted or what a later update
//! replaced ([`CloudConnectorRecord::apply_update`]): the ARN is derived from the id and
//! the Region, `CreatedAt` is when the create ran and `UpdatedAt` is when the last write
//! to that connector ran.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    CloudConnectorConfiguration, CloudConnectorSummary, ConfigurationTargets,
    GcpConfigurationTargets,
};

/// The resource portion a cloud connector's ARN carries before the connector's id, from
/// the modelled `CloudConnectorArn` pattern
/// `^arn:aws(-cn|-us-gov)?:ssm:([^:]+):\d{12}:cloud-connector/[0-9a-f]{8}-…$`.
pub const ARN_CLOUD_CONNECTOR_RESOURCE_PREFIX: &str = "cloud-connector/";

/// How this service reports a `CloudConnectorId` the account holds no cloud connector
/// for, through `ResourceNotFoundException`.
///
/// The whole sentence is FIXED text for this error class: it names the resource class
/// and interpolates nothing from the request, so the id a caller submitted does not
/// appear in it. Every operation that resolves a connector id reports it, so a read and
/// a write of the same absent connector are worded the same way.
pub const CLOUD_CONNECTOR_NOT_FOUND: &str = "CloudConnector not found for account.";

/// A fresh `CloudConnectorId`: the id of a connector being created.
///
/// `CloudConnectorId` is a 36-character lowercase-hex UUID
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) the SERVICE
/// chooses — no input member carries one into a create — so it is drawn from the crate's
/// one identifier source ([`super::new_uuid`], a random version-4 identifier) rather
/// than from anything in the request, and two creates of identical requests are two
/// connectors.
pub fn new_cloud_connector_id() -> String {
    super::new_uuid()
}

/// The kind of resource a cloud connector is, as this service names it: the
/// `CloudConnector` member of the modelled `ResourceTypeForTagging` value set
/// (`crate::ops::types::RESOURCE_TYPE_FOR_TAGGING_VALUES`).
pub const CLOUD_CONNECTOR_RESOURCE_TYPE: &str = "CloudConnector";

/// How many cloud connectors this store holds at once, across every Region.
///
/// This is the EMULATOR's own ceiling rather than a quota Systems Manager publishes:
/// a create is the only thing that admits a connector, callers may run as many as they
/// like, and the store is held in memory for as long as the process runs — so without a
/// ceiling the one map would grow for as long as creates keep arriving. A create past it
/// is refused through the modelled `ServiceQuotaExceededException` and writes nothing.
pub const MAX_CLOUD_CONNECTORS: usize = 100;

/// What a create refused by [`MAX_CLOUD_CONNECTORS`] reports: the emulator's own ceiling,
/// named as the emulator's, with the figure it stands at.
pub fn cloud_connector_ceiling_message() -> String {
    format!("This emulator holds at most {MAX_CLOUD_CONNECTORS} cloud connectors across every Region")
}

// ---------------------------------------------------------------------------
// Narrowing and paging a cloud connector listing
// ---------------------------------------------------------------------------

/// The `CloudConnectorFilterKey` that narrows a listing to the connectors targeting one
/// Azure subscription (`crate::ops::types::CLOUD_CONNECTOR_FILTER_KEY_VALUES`): "The Azure
/// subscription ID to filter by. To return only tenant-level connectors, specify `NONE`"
/// (`CloudConnectorFilter$FilterValues`).
pub const CLOUD_CONNECTOR_FILTER_KEY_SUBSCRIPTION_ID: &str = "SubscriptionId";

/// The `CloudConnectorFilterKey` that narrows a listing to the connectors of one Azure
/// tenant (same enum): "The Azure tenant ID to filter by. Filters the results to connectors
/// that target the specified tenant."
pub const CLOUD_CONNECTOR_FILTER_KEY_TENANT_ID: &str = "TenantId";

/// The `CloudConnectorFilterKey` that narrows a listing to the connectors targeting one
/// Google Cloud Platform project (same enum): "The Google Cloud Platform (GCP) project ID to
/// filter by. Filters the results to connectors that target the specified project."
pub const CLOUD_CONNECTOR_FILTER_KEY_PROJECT_ID: &str = "ProjectId";

/// The `CloudConnectorFilterKey` that narrows a listing to the connectors of one GCP
/// organization (same enum): "The GCP organization ID to filter by. Filters the results to
/// connectors that target the entire specified organization."
pub const CLOUD_CONNECTOR_FILTER_KEY_ORGANIZATION_ID: &str = "OrganizationId";

/// The filter keys that narrow a listing by a property of an AZURE configuration
/// ([`CLOUD_CONNECTOR_FILTER_KEY_SUBSCRIPTION_ID`],
/// [`CLOUD_CONNECTOR_FILTER_KEY_TENANT_ID`]).
pub const AZURE_CLOUD_CONNECTOR_FILTER_KEYS: &[&str] = &[
    CLOUD_CONNECTOR_FILTER_KEY_SUBSCRIPTION_ID,
    CLOUD_CONNECTOR_FILTER_KEY_TENANT_ID,
];

/// The filter keys that narrow a listing by a property of a GCP configuration
/// ([`CLOUD_CONNECTOR_FILTER_KEY_PROJECT_ID`],
/// [`CLOUD_CONNECTOR_FILTER_KEY_ORGANIZATION_ID`]).
pub const GCP_CLOUD_CONNECTOR_FILTER_KEYS: &[&str] = &[
    CLOUD_CONNECTOR_FILTER_KEY_PROJECT_ID,
    CLOUD_CONNECTOR_FILTER_KEY_ORGANIZATION_ID,
];

/// What a listing that narrows by an Azure filter key and a GCP filter key at once is
/// refused with: "You can't combine Azure filter keys (`SubscriptionId` and `TenantId`) with
/// GCP filter keys (`ProjectId` and `OrganizationId`) in the same request"
/// (`CloudConnectorFilter$FilterValues`). One connector has a configuration of ONE provider,
/// so the two families name properties no connector has at once.
///
/// The whole sentence is FIXED text for this condition: it names the two families and
/// interpolates nothing from the request.
pub const CLOUD_CONNECTOR_FILTER_KEYS_NOT_COMBINABLE: &str =
    "Azure filter keys (SubscriptionId, TenantId) can't be combined with GCP filter keys \
     (ProjectId, OrganizationId) in the same request";

/// Whether `keys` names a filter key of BOTH families, which is what
/// [`CLOUD_CONNECTOR_FILTER_KEYS_NOT_COMBINABLE`] refuses.
pub fn filter_keys_are_not_combinable<'a, I: IntoIterator<Item = &'a str>>(keys: I) -> bool {
    let (mut azure, mut gcp) = (false, false);
    for key in keys {
        azure |= AZURE_CLOUD_CONNECTOR_FILTER_KEYS.contains(&key);
        gcp |= GCP_CLOUD_CONNECTOR_FILTER_KEYS.contains(&key);
    }
    azure && gcp
}

/// The `Provider` of a connector whose configuration targets Microsoft Azure: a value of the
/// modelled `Provider` enum (`crate::ops::types::PROVIDER_VALUES`), which is what a listing
/// reports as the "third-party cloud provider that the cloud connector connects to".
pub const PROVIDER_AZURE: &str = "Azure";

/// The `Provider` of a connector whose configuration targets Google Cloud Platform. The
/// other value of the same enum.
pub const PROVIDER_GCP: &str = "GCP";

/// The `SubscriptionId` filter value that asks for the connectors targeting NO subscription
/// at all: "To return only tenant-level connectors, specify `NONE`"
/// (`CloudConnectorFilter$FilterValues`). It is a keyword of the filter rather than a
/// subscription id, which the modelled `AzureSubscriptionId` pattern could not take.
pub const TENANT_LEVEL_SUBSCRIPTION_FILTER_VALUE: &str = "NONE";

/// Whether `key` is a key a cloud connector listing filters on — a value of the modelled
/// `CloudConnectorFilterKey` enum.
pub fn is_cloud_connector_filter_key(key: &str) -> bool {
    crate::ops::types::CLOUD_CONNECTOR_FILTER_KEY_VALUES.contains(&key)
}

/// What a `Filters` entry naming no filter key at all is refused with. Both members of
/// `CloudConnectorFilter` are optional in the model, so such an entry reaches the handler,
/// and it names no property of a connector for the listing to be narrowed by.
pub const CLOUD_CONNECTOR_FILTER_KEY_MISSING: &str =
    "A Filters entry carries no filter key to search on";

/// What a `Filters` entry naming a key but no value is refused with, worded as the sibling
/// listings of this service word it ([`crate::ops::list_documents`]).
pub fn cloud_connector_filter_value_missing(key: &str) -> String {
    format!("Filter key '{key}' carries no value to search for")
}

/// The published ceiling of `CloudConnectorMaxResults` (range 0..10). It is also the page
/// size a request that bounds its page with no positive `MaxResults` is served: "The maximum
/// number of items to return for this call" bounds a page the caller asked to be bounded, so
/// a caller asking for no bound is served the whole page the operation allows.
pub const MAX_CLOUD_CONNECTOR_PAGE_SIZE: i32 = 10;

/// The smallest page a `MaxResults` can ask for: one connector. The modelled range reaches
/// down to zero because `CloudConnectorMaxResults` carries the shape default `0`, which is
/// the value a request naming no page size carries rather than a page of no connectors.
pub const MIN_CLOUD_CONNECTOR_PAGE_SIZE: i32 = 1;

/// The page size a cloud connector listing serves, or the refusal message for a `MaxResults`
/// outside the modelled range — which the protocol's boundary validator already refuses, so
/// this is a backstop rather than the first line of defence.
///
/// A request naming no `MaxResults`, and one naming the zero the shape defaults to, are both
/// served up to [`MAX_CLOUD_CONNECTOR_PAGE_SIZE`]: neither asked for a narrower page.
pub fn cloud_connector_page_size(max_results: Option<i32>) -> Result<usize, String> {
    match max_results {
        None | Some(0) => Ok(MAX_CLOUD_CONNECTOR_PAGE_SIZE as usize),
        requested => super::page_size(
            requested,
            MIN_CLOUD_CONNECTOR_PAGE_SIZE,
            MAX_CLOUD_CONNECTOR_PAGE_SIZE,
        ),
    }
}

/// Whether `token` is shaped like a `CloudConnectorId`, which is what a listing hands out as
/// its `NextToken`: the hyphenated lowercase-hex UUID this store mints
/// ([`new_cloud_connector_id`]), read by the one reader of that shape this crate has
/// ([`super::is_activation_id`], which is that shape test and nothing more).
pub fn is_cloud_connector_id(token: &str) -> bool {
    super::is_activation_id(token)
}

/// Which third-party cloud a `CloudConnectorConfiguration` connects to, as the modelled
/// `Provider` enum names it: the provider whose member of the union the configuration carries
/// ([`PROVIDER_AZURE`], [`PROVIDER_GCP`]).
///
/// Read off the CONFIGURATION rather than off a stored connector, so a create resolves the
/// provider of the configuration it was handed before any connector exists to read it from;
/// [`CloudConnectorRecord::provider`] is this over the one a record holds.
pub fn configuration_provider(configuration: &CloudConnectorConfiguration) -> &'static str {
    match configuration {
        CloudConnectorConfiguration::AzureConfiguration(_) => PROVIDER_AZURE,
        CloudConnectorConfiguration::GcpConfiguration(_) => PROVIDER_GCP,
    }
}

/// The Azure tenant a `CloudConnectorConfiguration` targets and the application registration
/// it authenticates as there — `AzureConfiguration$TenantId` ("The ID of the Azure tenant")
/// and `$ApplicationId` ("The ID of the Azure application registration used for
/// authentication"), which an Azure configuration always carries because both members are
/// `@required`.
///
/// `None` for a configuration of ANOTHER provider: a GCP configuration names neither, so
/// there is no Azure pair to put the provider-credential question about
/// ([`super::provider_credentials`]).
///
/// Read off the CONFIGURATION rather than off a stored connector, for the reason
/// [`configuration_provider`] is; [`CloudConnectorRecord::azure_identity`] is this over the
/// one a record holds.
pub fn configuration_azure_identity(
    configuration: &CloudConnectorConfiguration,
) -> Option<(&str, &str)> {
    match configuration {
        CloudConnectorConfiguration::AzureConfiguration(azure) => {
            Some((azure.tenant_id.as_str(), azure.application_id.as_str()))
        }
        CloudConnectorConfiguration::GcpConfiguration(_) => None,
    }
}

/// One cloud connector, as the connector reads report it.
#[derive(Debug, Clone)]
pub struct CloudConnectorRecord {
    /// The server-generated id this record is stored under.
    pub id: String,
    /// The Region whose endpoint this connector was created through, and which its ARN
    /// carries.
    pub region: String,
    /// "A friendly name for the cloud connector" — required on create, so a connector
    /// always has one.
    pub display_name: String,
    /// "A description for the cloud connector", or `None` for a connector created
    /// without one.
    pub description: Option<String>,
    /// "The Amazon Resource Name (ARN) of the IAM role that the cloud connector uses to
    /// communicate with the third-party cloud environment."
    pub role_arn: String,
    /// "The ARN of the Amazon Web Services Config connector associated with this cloud
    /// connector", or `None` for a connector created without one — the member is optional on
    /// the create, and a read "returns this value only if the cloud connector was created
    /// with an Amazon Web Services Config connector".
    pub config_connector_arn: Option<String>,
    /// "The configuration details for connecting to the third-party cloud environment."
    pub configuration: CloudConnectorConfiguration,
    /// The connector's tags, keyed by tag key, as the create supplied them.
    pub tags: BTreeMap<String, String>,
    /// "The date and time the cloud connector was created."
    pub created_at: SystemTime,
    /// "The date and time the cloud connector was last updated." Equal to
    /// [`created_at`](Self::created_at) until something updates the connector.
    pub updated_at: SystemTime,
}

impl CloudConnectorRecord {
    /// This connector's ARN: `arn:aws:ssm:<region>:<account>:cloud-connector/<id>`.
    pub fn arn(&self) -> String {
        ::engine::emulator_arn(
            super::ARN_SERVICE,
            &self.region,
            &format!("{ARN_CLOUD_CONNECTOR_RESOURCE_PREFIX}{}", self.id),
        )
    }

    /// Apply an update to this connector and stamp `now` as when it was last updated.
    ///
    /// Each member an update carries REPLACES the stored one and each member it omits
    /// leaves the stored one alone: `DisplayName`, `Description` and `Configuration` are
    /// all optional on the update, which is what makes an update of one member not a
    /// create of the others. The id, the Region, the IAM role, the Config connector ARN,
    /// the tags and the creation instant are not members of the update at all, so no
    /// update changes them.
    pub fn apply_update(
        &mut self,
        display_name: Option<String>,
        description: Option<String>,
        configuration: Option<CloudConnectorConfiguration>,
        now: SystemTime,
    ) {
        if let Some(display_name) = display_name {
            self.display_name = display_name;
        }
        if let Some(description) = description {
            self.description = Some(description);
        }
        if let Some(configuration) = configuration {
            self.configuration = configuration;
        }
        self.updated_at = now;
    }

    /// Which third-party cloud this connector connects to, as the modelled `Provider` enum
    /// names it ([`configuration_provider`]): the provider of the configuration it holds.
    pub fn provider(&self) -> &'static str {
        configuration_provider(&self.configuration)
    }

    /// The Azure tenant this connector targets and the application it authenticates as
    /// there, or `None` for a connector of another provider
    /// ([`configuration_azure_identity`]).
    pub fn azure_identity(&self) -> Option<(&str, &str)> {
        configuration_azure_identity(&self.configuration)
    }

    /// The GCP organization this connector targets in full, in the order its configuration
    /// names it — `GcpConfiguration$OrganizationId`, "The ID of the GCP organization", which
    /// is optional, and `None` for a connector of another provider.
    pub fn organization_id(&self) -> Option<&str> {
        match &self.configuration {
            CloudConnectorConfiguration::GcpConfiguration(gcp) => gcp.organization_id.as_deref(),
            CloudConnectorConfiguration::AzureConfiguration(_) => None,
        }
    }

    /// The GCP projects this connector targets, in the order its configuration lists them.
    ///
    /// Empty for a connector whose configuration names none — `GcpConfiguration$Targets` is
    /// optional ("The target GCP projects for the cloud connector") — and for a connector of
    /// another provider, the way [`Self::subscription_ids`] is empty for this one.
    pub fn project_ids(&self) -> Vec<&str> {
        match &self.configuration {
            CloudConnectorConfiguration::GcpConfiguration(gcp) => match &gcp.targets {
                Some(GcpConfigurationTargets::Projects(projects)) => projects
                    .iter()
                    .map(|project| project.project_id.as_str())
                    .collect(),
                None => Vec::new(),
            },
            CloudConnectorConfiguration::AzureConfiguration(_) => Vec::new(),
        }
    }

    /// The subscriptions this connector targets, in the order its configuration lists them.
    ///
    /// Empty for a TENANT-LEVEL connector: `AzureConfiguration$Targets` is optional ("The
    /// target Azure subscriptions for the cloud connector"), so a configuration that names
    /// none targets the tenant alone — which is what the `SubscriptionId` filter's `NONE`
    /// asks for ([`TENANT_LEVEL_SUBSCRIPTION_FILTER_VALUE`]).
    pub fn subscription_ids(&self) -> Vec<&str> {
        match &self.configuration {
            CloudConnectorConfiguration::AzureConfiguration(azure) => match &azure.targets {
                Some(ConfigurationTargets::Subscriptions(subscriptions)) => subscriptions
                    .iter()
                    .map(|subscription| subscription.id.as_str())
                    .collect(),
                None => Vec::new(),
            },
            CloudConnectorConfiguration::GcpConfiguration(_) => Vec::new(),
        }
    }

    /// Whether this connector meets the filter `key` carrying `values`: the connector is
    /// reported when ANY value of the filter names what the key filters on, so one filter is
    /// one criterion however many values it carries.
    ///
    /// `TenantId` holds the value against the tenant the connector's configuration targets;
    /// `SubscriptionId` holds it against the subscriptions it targets, with `NONE` asking
    /// for the connectors that target none of them; `OrganizationId` and `ProjectId` hold it
    /// the same way against a GCP configuration's organization and its targeted projects. A
    /// key of one provider's family names no property of a connector of ANOTHER provider, so
    /// no such connector meets it. A key outside the four the model declares names no
    /// property of a connector at all, so nothing meets it — the boundary refuses such a key
    /// before a listing reaches this.
    pub fn matches_filter(&self, key: &str, values: &[String]) -> bool {
        values.iter().any(|value| match key {
            CLOUD_CONNECTOR_FILTER_KEY_TENANT_ID => self
                .azure_identity()
                .is_some_and(|(tenant_id, _)| tenant_id == value.as_str()),
            CLOUD_CONNECTOR_FILTER_KEY_SUBSCRIPTION_ID => {
                let targeted = self.subscription_ids();
                if value == TENANT_LEVEL_SUBSCRIPTION_FILTER_VALUE {
                    // "To return only TENANT-level connectors": a connector of another
                    // provider targets no Azure tenant at all, so it is not one of them.
                    self.provider() == PROVIDER_AZURE && targeted.is_empty()
                } else {
                    targeted.contains(&value.as_str())
                }
            }
            CLOUD_CONNECTOR_FILTER_KEY_ORGANIZATION_ID => self
                .organization_id()
                .is_some_and(|organization_id| organization_id == value.as_str()),
            CLOUD_CONNECTOR_FILTER_KEY_PROJECT_ID => self.project_ids().contains(&value.as_str()),
            _ => false,
        })
    }

    /// This connector as a listing reports it: the `CloudConnectorSummary` members, each
    /// read out of what the create persisted and the last update replaced.
    ///
    /// A summary is narrower than the single-connector read: it carries neither the ARN nor
    /// the configuration, which `CloudConnectorSummary` does not declare. It does carry the
    /// `Provider` the read does not, which is the provider of the configuration the read
    /// reports in full ([`Self::provider`]).
    pub fn to_summary(&self) -> CloudConnectorSummary {
        CloudConnectorSummary {
            cloud_connector_id: Some(self.id.clone()),
            display_name: Some(self.display_name.clone()),
            description: self.description.clone(),
            role_arn: Some(self.role_arn.clone()),
            created_at: Some(crate::state::wire_instant(self.created_at)),
            updated_at: Some(crate::state::wire_instant(self.updated_at)),
            provider: Some(self.provider().to_string()),
            config_connector_arn: self.config_connector_arn.clone(),
        }
    }
}

impl super::ComAmazonawsSsmAmazonssmState {
    /// The cloud connector `id` names in `region`, or `None` when that Region holds
    /// none — which is what the not-found refusal reports.
    pub fn cloud_connector(&self, region: &str, id: &str) -> Option<&CloudConnectorRecord> {
        self.cloud_connectors
            .get(&(region.to_string(), id.to_string()))
    }

    /// The same record, to apply an update to it.
    pub fn cloud_connector_mut(
        &mut self,
        region: &str,
        id: &str,
    ) -> Option<&mut CloudConnectorRecord> {
        self.cloud_connectors
            .get_mut(&(region.to_string(), id.to_string()))
    }

    /// Store `record` as a cloud connector of its own Region, keyed by its id.
    pub fn insert_cloud_connector(&mut self, record: CloudConnectorRecord) {
        let key = (record.region.clone(), record.id.clone());
        self.cloud_connectors.insert(key, record);
    }

    /// Drop the cloud connector `id` names in `region`, answering whether one was held —
    /// which is what tells a delete that removed a connector from one whose id names none.
    pub fn remove_cloud_connector(&mut self, region: &str, id: &str) -> bool {
        self.cloud_connectors
            .remove(&(region.to_string(), id.to_string()))
            .is_some()
    }

    /// Every cloud connector `region` holds, in ascending connector-id order.
    ///
    /// The order is what makes a listing's resume cursor total: the token a page hands back
    /// is the id of the connector the next page starts at, not an offset into a list that
    /// may have grown or shrunk between two requests.
    pub fn cloud_connectors_in_region<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a CloudConnectorRecord> + 'a {
        self.cloud_connectors
            .iter()
            .filter(move |((held, _), _)| held == region)
            .map(|(_, record)| record)
    }
}
