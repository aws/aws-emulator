// SPDX-License-Identifier: CC0-1.0

//! The Bicep setup artifact `GetConnectorTelemetrySetup` answers, RENDERED from the
//! connector and the telemetry manifest it carries.
//!
//! The operation "generates the artifacts that you use to create the infrastructure in
//! your cloud provider that delivers the telemetry declared in the connector's
//! telemetry manifest", and it "doesn't change any resources in your cloud provider or
//! in Amazon Web Services" — so the artifact is a pure function of the stored connector
//! and manifest, and this module is that function. Nothing here reaches into Azure:
//! every name, scope, Region, resource type and size in the document comes from the
//! manifest the caller wrote and the connector it belongs to.
//!
//! The document is Bicep, the one format `SetupFormat` carries, and it declares the
//! three things the manifest asks the setup for:
//!
//! 1. the Event Hubs the `ResourceLogs` of the manifest's scope are delivered through,
//!    sized by `AzureInfrastructure$EventHubs` (its `Tier` is the namespace SKU and its
//!    `PartitionCount` the hub's partitions);
//! 2. where the `FlowLogs` of that scope are read from: the storage accounts the
//!    manifest names for a `CustomerManaged` location, or the account the setup creates
//!    with the retention the manifest asks for when the location is `AwsManaged`;
//! 3. the role assignment that lets the connector's own Azure AD application read the
//!    scope, which is why applying the artifact needs an Azure principal that can
//!    assign roles.
//!
//! A manifest that asks for none of those still renders a document: its scope and
//! telemetry types are what the diagnostic settings the setup configures are derived
//! from, and they are declared as variables so the applied document records exactly
//! what was asked for.
//!
//! ## The size of the document is the SERVER's decision
//!
//! A stored manifest is caller-sized: its scope may name a hundred items, each with a
//! thousand subscription ids and a hundred thousand resource ids. So the one part of
//! the document whose length follows those lists — the scope section — is rendered
//! against a budget ([`MAX_SCOPE_SECTION_CHARS`]) that is re-checked before EVERY value
//! it appends, not once per scope item: a single list cannot take the section past the
//! budget however long it is. Nothing is collected into an intermediate list first
//! either; each value is rendered straight into the section as it is read. What did not
//! fit is counted and reported in the document, so a reader is never told the setup
//! covers more than it does.

use super::*;

/// The file name the artifact carries, which `SetupArtifact$FileName` documents as
/// "the suggested file name, for example `main.bicep`".
pub const SETUP_ARTIFACT_FILE_NAME: &str = "main.bicep";

/// The content type the artifact carries. A Bicep document is source text, and
/// `SetupArtifact$ContentType` documents the member with "for example `text/plain`".
pub const SETUP_ARTIFACT_CONTENT_TYPE: &str = "text/plain";

/// The one value `SetupFormat` carries (`SETUP_FORMAT_VALUES`), which is the default
/// format for an Azure connector — "if you omit this value, the service uses the
/// default format for the connector's provider (BICEP for an Azure connector)".
pub const SETUP_FORMAT_BICEP: &str = "BICEP";

/// The Azure role an operator needs to CREATE the resources the artifact declares.
/// Named by this member's own documentation: "the permissions or roles in your cloud
/// provider that you need to apply the artifacts, for example Azure Contributor and
/// User Access Administrator".
pub const AZURE_ROLE_CONTRIBUTOR: &str = "Contributor";

/// The Azure role an operator needs to ASSIGN the connector's application a role,
/// which the artifact does whenever it grants the connector read access to the
/// manifest's scope. The second role this member's documentation names.
pub const AZURE_ROLE_USER_ACCESS_ADMINISTRATOR: &str = "User Access Administrator";

/// The Azure built-in role the artifact assigns the connector's application: the
/// connector only READS the provider's resources and their telemetry, so the role it
/// is granted is Azure's own read-only one.
const AZURE_READER_ROLE_DEFINITION_ID: &str = "acdd72a7-3385-48ef-bd42-f606fba81ae7";

/// How many characters of the connector id a generated Azure resource name carries.
///
/// An Event Hubs namespace name is globally unique and bounded well below the 32
/// characters of this service's own id, so the generated name carries this many of them
/// — enough that two connectors of one account never collide, short enough to leave
/// room for the prefix and the index.
const NAME_ID_CHARS: usize = 12;

/// The prefix every Azure resource the artifact declares carries, so an operator can
/// tell what created it.
const RESOURCE_NAME_PREFIX: &str = "cwobs";

/// The most characters the SCOPE SECTION of the document may carry.
///
/// `SetupArtifactContent`'s own modelled 0..262144 length is what one artifact may be,
/// and the scope section is the only part of the document whose length follows the
/// manifest's lists rather than a fixed shape — so this is the budget it is held to,
/// leaving the fixed header and the bounded resource declarations room inside that
/// maximum. It is re-checked before every value the section appends ([`push_var`]), so
/// the section's length is this constant at most whatever the stored manifest holds.
const MAX_SCOPE_SECTION_CHARS: usize = 192 * 1024;

/// The artifacts the setup of the connector `id` consists of, and the roles needed to
/// apply them.
///
/// A connector that carries no telemetry manifest has asked for no infrastructure, so
/// it has NO artifacts and needs no permissions: `Artifacts` is modelled with a minimum
/// length of 0 for exactly that state, and `ManifestVersion` — "the version of the
/// telemetry manifest that the service generated the setup artifacts from" — is absent
/// because no manifest was read.
///
/// Every input is BORROWED rather than read from the store: the caller hands over the
/// few connector fields and the manifest behind its `Arc`, so the render runs with no
/// lock held on this service's shared state.
pub fn setup(
    id: &str,
    configuration: &ConnectorConfiguration,
    manifest: Option<(&TelemetryManifest, i64)>,
) -> (Vec<SetupArtifact>, Vec<String>) {
    let Some((manifest, version)) = manifest else {
        return (Vec::new(), Vec::new());
    };
    let artifact = SetupArtifact {
        file_name: SETUP_ARTIFACT_FILE_NAME.to_string(),
        content: render(id, configuration, manifest, version),
        format: SETUP_FORMAT_BICEP.to_string(),
        content_type: Some(SETUP_ARTIFACT_CONTENT_TYPE.to_string()),
    };
    (
        vec![artifact],
        vec![
            AZURE_ROLE_CONTRIBUTOR.to_string(),
            AZURE_ROLE_USER_ACCESS_ADMINISTRATOR.to_string(),
        ],
    )
}

/// The Bicep document for the connector `id` at manifest `version`.
fn render(
    id: &str,
    configuration: &ConnectorConfiguration,
    manifest: &TelemetryManifest,
    version: i64,
) -> String {
    let TelemetryManifestSource::Azure(scope_items) = &manifest.source;
    let infrastructure = manifest
        .infrastructure
        .as_ref()
        .map(|TelemetryManifestInfrastructure::Azure(azure)| azure);
    let ConnectorConfiguration::Azure(azure) = configuration;
    // Every value embedded in a Bicep literal below goes through [`escape`], whether or
    // not the model's own pattern already bounds it, so no embedded value can change
    // what the document means.
    let tenant = escape(&azure.tenant_identifier);
    let client = escape(&azure.client_identifier);
    let handle: String = id.chars().take(NAME_ID_CHARS).collect();

    let mut out = String::new();
    out.push_str(&format!(
        "// Telemetry setup for the Amazon CloudWatch Observability Admin connector \
         {id}.\n// Rendered from telemetry manifest version {version}.\n\n"
    ));
    out.push_str("targetScope = 'subscription'\n\n");
    out.push_str(&format!(
        "@description('The Azure AD tenant the connector reads.')\nparam \
         connectorTenantId string = '{tenant}'\n\n@description('The Azure AD \
         application the connector authenticates as.')\nparam connectorClientId \
         string = '{client}'\n\n"
    ));

    out.push_str(&scope_section(scope_items));

    let subscription = infrastructure
        .and_then(|azure| azure.subscription_id.as_deref())
        .map(escape)
        .unwrap_or_default();
    if !subscription.is_empty() {
        out.push_str(&format!(
            "@description('The Azure subscription the setup creates the telemetry \
             infrastructure in.')\nparam infrastructureSubscriptionId string = \
             '{subscription}'\n\n"
        ));
    }

    // The Event Hubs the manifest sized. `AzureInfrastructure$EventHubs` admits exactly
    // one entry, which "applies to the Event Hubs in every Azure Region", so the
    // namespace is declared once and the deployment's own location is where it lands.
    for (index, hub) in infrastructure
        .and_then(|azure| azure.event_hubs.as_deref())
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        out.push_str(&format!(
            "resource eventHubNamespace{index} \
             'Microsoft.EventHub/namespaces@2024-01-01' = {{\n  name: \
             '{RESOURCE_NAME_PREFIX}-{handle}-{index}'\n  location: \
             deployment().location\n  sku: {{\n    name: '{tier}'\n    tier: \
             '{tier}'\n  }}\n}}\n\nresource eventHub{index} \
             'Microsoft.EventHub/namespaces/eventhubs@2024-01-01' = {{\n  parent: \
             eventHubNamespace{index}\n  name: 'telemetry'\n  properties: {{\n    \
             partitionCount: {partitions}\n  }}\n}}\n\n",
            tier = escape(&hub.tier),
            partitions = hub.partition_count
        ));
    }

    // Where the Flow Logs of the scope are read from.
    match infrastructure.and_then(|azure| azure.flow_logs.as_ref()) {
        Some(AzureFlowLogsInfrastructure::CustomerManaged(managed)) => {
            out.push_str(
                "// Flow Logs are read from the storage accounts the manifest names.\n",
            );
            let mut section = String::new();
            let omitted = push_var(
                &mut section,
                "flowLogStorageAccountIds",
                managed
                    .storage_accounts
                    .iter()
                    .flat_map(|scope| scope.storage_account_ids.iter().map(String::as_str)),
            );
            out.push_str(&section);
            out.push_str(&omission_note(omitted, "storage account ids"));
            out.push('\n');
        }
        Some(AzureFlowLogsInfrastructure::AwsManaged(managed)) => {
            out.push_str(&format!(
                "resource flowLogStorage \
                 'Microsoft.Storage/storageAccounts@2023-05-01' = {{\n  name: \
                 '{RESOURCE_NAME_PREFIX}{handle}'\n  location: \
                 deployment().location\n  kind: 'StorageV2'\n  sku: {{\n    name: \
                 'Standard_LRS'\n  }}\n}}\n\nresource flowLogRetention \
                 'Microsoft.Storage/storageAccounts/managementPolicies@2023-05-01' = \
                 {{\n  parent: flowLogStorage\n  name: 'default'\n  properties: {{\n    \
                 policy: {{\n      rules: [\n        {{\n          name: \
                 'flowLogRetention'\n          type: 'Lifecycle'\n          \
                 definition: {{\n            actions: {{\n              baseBlob: \
                 {{\n                delete: {{\n                  \
                 daysAfterModificationGreaterThan: {days}\n                }}\n       \
                        }}\n            }}\n          }}\n        }}\n      ]\n    \
                 }}\n  }}\n}}\n\n",
                days = managed.retention_days
            ));
        }
        None => {}
    }

    // The connector's application reads the scope, and nothing more: the role it is
    // assigned is Azure's own read-only built-in role.
    out.push_str(&format!(
        "resource connectorReader \
         'Microsoft.Authorization/roleAssignments@2022-04-01' = {{\n  name: \
         guid(subscription().id, connectorClientId, \
         '{AZURE_READER_ROLE_DEFINITION_ID}')\n  properties: {{\n    \
         roleDefinitionId: \
         subscriptionResourceId('Microsoft.Authorization/roleDefinitions', \
         '{AZURE_READER_ROLE_DEFINITION_ID}')\n    principalId: \
         connectorClientId\n    principalType: 'ServicePrincipal'\n  }}\n}}\n"
    ));

    out
}

/// The scope section: one group of variables per scope item, so the applied document
/// records exactly which resources the telemetry was asked for.
///
/// Held to [`MAX_SCOPE_SECTION_CHARS`] by construction — [`push_var`] re-checks the
/// section's length before every value it appends — so the section's length is decided
/// here and never by the size of the stored manifest. What did not fit is counted and
/// stated at the end of the section.
fn scope_section(scope_items: &[AzureScopeItem]) -> String {
    let mut section = String::new();
    let mut omitted_values = 0usize;
    let mut listed = 0usize;

    for (index, item) in scope_items.iter().enumerate() {
        let before = section.len();
        omitted_values += push_var(
            &mut section,
            &format!("telemetryTypes{index}"),
            item.telemetry_types.iter().map(String::as_str),
        );
        omitted_values += push_var(
            &mut section,
            &format!("subscriptions{index}"),
            subscription_ids(&item.scope),
        );
        if let Some(regions) = item.scope.regions.as_ref() {
            omitted_values += push_var(
                &mut section,
                &format!("regions{index}"),
                regions.iter().map(String::as_str),
            );
        }
        if let Some(resource_types) = item.scope.resource_types.as_ref() {
            omitted_values += push_var(
                &mut section,
                &format!("resourceTypes{index}"),
                resource_types.iter().map(String::as_str),
            );
        }
        if let Some(resources) = item.scope.resources.as_ref() {
            omitted_values += push_var(
                &mut section,
                &format!("resourceIds{index}"),
                resources
                    .iter()
                    .flat_map(|scope| scope.resource_ids.iter().map(String::as_str)),
            );
        }
        if let Some(filters) = item.scope.tag_filters.as_ref() {
            // A tag filter's key is not an identifier, so the keys are a list of their
            // own and each filter's values are a variable named by its position in it.
            omitted_values += push_var(
                &mut section,
                &format!("tagFilterKeys{index}"),
                filters.iter().map(|filter| filter.key.as_str()),
            );
            for (slot, filter) in filters.iter().enumerate() {
                omitted_values += push_var(
                    &mut section,
                    &format!("tagFilterValues{index}_{slot}"),
                    filter.values.iter().map(String::as_str),
                );
            }
        }
        // Nothing of this item fit, which means the budget is spent: stop rather than
        // walking the rest of a manifest that cannot be reported.
        if section.len() == before {
            break;
        }
        if section.len() < MAX_SCOPE_SECTION_CHARS {
            section.push('\n');
        }
        listed += 1;
    }

    if listed < scope_items.len() {
        section.push_str(&format!(
            "// {} of the {} scope items the telemetry manifest declares are not listed \
             above: one artifact carries at most {MAX_SCOPE_SECTION_CHARS} characters of \
             scope.\n",
            scope_items.len() - listed,
            scope_items.len()
        ));
    }
    section.push_str(&omission_note(omitted_values, "values"));
    if !section.is_empty() {
        section.push('\n');
    }
    section
}

/// The comment that states how many values a budget left out, and nothing when none
/// were.
fn omission_note(omitted: usize, what: &str) -> String {
    if omitted == 0 {
        return String::new();
    }
    format!(
        "// {omitted} {what} the telemetry manifest declares are not listed above: one \
         artifact carries at most {MAX_SCOPE_SECTION_CHARS} characters of scope.\n"
    )
}

/// Append `var <name> = [ … ]` to `section` and answer how many of `values` were left
/// out.
///
/// The budget is checked before EVERY entry, and against the whole declaration before
/// anything is appended, so neither the number of entries nor the length of one can
/// take `section` past [`MAX_SCOPE_SECTION_CHARS`]. The values are read through an
/// iterator and rendered straight into the declaration, so no intermediate list of them
/// is built: a manifest naming a hundred thousand resource ids costs one bounded string
/// here and not a copy of all of them.
fn push_var<'a>(
    section: &mut String,
    name: &str,
    values: impl IntoIterator<Item = &'a str>,
) -> usize {
    // The closing bracket and newline must still fit after whatever is appended, so
    // they are reserved up front.
    const CLOSING: usize = "]\n".len();
    let mut declaration = format!("var {name} = [");
    let mut omitted = 0usize;
    let mut spent = false;
    let mut first = true;

    for value in values {
        if spent {
            omitted += 1;
            continue;
        }
        let separator = if first { "" } else { ", " };
        let escaped = escape(value);
        let entry_len = separator.len() + escaped.len() + 2;
        if section
            .len()
            .saturating_add(declaration.len())
            .saturating_add(entry_len)
            .saturating_add(CLOSING)
            > MAX_SCOPE_SECTION_CHARS
        {
            spent = true;
            omitted += 1;
            continue;
        }
        declaration.push_str(separator);
        declaration.push('\'');
        declaration.push_str(&escaped);
        declaration.push('\'');
        first = false;
    }

    declaration.push_str("]\n");
    if section.len().saturating_add(declaration.len()) <= MAX_SCOPE_SECTION_CHARS {
        section.push_str(&declaration);
    }
    omitted
}

/// `value` as the body of a single-quoted Bicep string literal.
///
/// Defence in depth, not a reachable path: every manifest member this document embeds is
/// bounded by the model — a `@pattern` that excludes `'`, `$`, `{`, `}`, `\`, `<`, `>`,
/// `"` and the control characters, or an enum's own value set — so the centralized
/// constraint walk refuses such a value long before it is stored
/// (`tests/all/success_response_projection.rs` pins that). What this keeps true is the
/// RENDERER's own invariant, independent of those patterns: a single-quoted Bicep literal
/// gives four of those characters a meaning of its own — a backslash starts an escape, an
/// apostrophe ends the literal, `${` opens an interpolation, and a literal newline,
/// carriage return or tab cannot appear in one at all — so each is written as the escape
/// Bicep reads it back from, and a value carrying any of them renders as itself instead
/// of changing what the document means or making it unparseable.
///
/// The backslash goes first, so the escapes the later replacements introduce are not
/// escaped again.
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('$', "\\$")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// The subscription ids a scope names, and none when it names them all: the `All`
/// variant is not a list of ids, and the document declares what the manifest asked for
/// rather than enumerating a tenant this emulator cannot read.
fn subscription_ids(scope: &AzureScope) -> impl Iterator<Item = &str> {
    let ids = match scope.subscriptions.as_ref() {
        Some(AzureSubscriptionScope::Ids(ids)) => Some(ids),
        Some(AzureSubscriptionScope::All(_)) | None => None,
    };
    ids.into_iter().flatten().map(String::as_str)
}
