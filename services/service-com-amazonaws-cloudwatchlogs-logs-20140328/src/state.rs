// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudwatchlogs-logs-20140328 service.
//!
//! The log-group / log-stream / log-event core loop is stored here, together with the
//! small helpers every handler of that loop shares: ARN construction, the wall clock,
//! log-group identifier resolution and the opaque pagination tokens
//! `GetLogEvents` / `DescribeLogStreams` / `DescribeLogGroups` hand back.
//!
//! A log group's subscription filters live on the group they are "associated with"
//! ([`LogGroupRecord::subscription_filters`]), so deleting the group removes them too.
//!
//! A log group's metric filters live on the group too
//! ([`LogGroupRecord::metric_filters`]), because `PutMetricFilter` "creates or updates a
//! metric filter and associates it with the specified log group" — which is also what
//! makes the group's `metricFilterCount` ([`LogGroupRecord::metric_filter_count`]) a
//! derived value rather than a second copy of the same truth.
//!
//! A log group's log transformer lives on the group too
//! ([`LogGroupRecord::transformer`]), because `PutTransformer` creates one "for a single
//! log group" and nothing else identifies it.
//!
//! The account's resource policies live here too: an account-scoped one in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::account_resource_policies`], a
//! resource-scoped one on the log group it is attached to
//! ([`LogGroupRecord::resource_policy`]).
//!
//! The account's import tasks — the CloudTrail Lake imports `CreateImportTask` starts and
//! `DescribeImportTasks` reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::import_tasks`], each one an
//! [`ImportTaskRecord`] carrying the instant the clock owes it a terminal status.
//!
//! A log group's data protection policy lives on that group as well
//! ([`LogGroupRecord::data_protection_policy`]), because `PutDataProtectionPolicy`
//! "applies to only the specified log group": one group holds at most one, and deleting
//! the group takes its policy with it.
//!
//! A log group's own field index policy lives on the group too
//! ([`LogGroupRecord::index_policy`]), because a log-group level index policy is named by
//! nothing but the group it applies to — so one group holds at most one and deleting the
//! group takes it with it. The account-wide field index policies that apply where a group
//! holds none of its own are the account policies in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::account_policies`], and which log groups
//! one of those selects is [`account_policy_selects_log_group`].
//!
//! The account's saved CloudWatch Logs Insights query definitions live here
//! ([`ComAmazonawsCloudwatchlogsLogs20140328State::query_definitions`]) rather than on a
//! log group: a definition is identified only by its own `queryDefinitionId`, and the log
//! groups it names are a value it carries, not its owner.
//!
//! A log group's syslog configurations live on the group as well
//! ([`LogGroupRecord::syslog_configurations`]), because `PutSyslogConfiguration` "creates
//! or updates a syslog configuration for a log group" and the only other thing that names
//! one is the VPC endpoint it ingests through — so the group keys them by that endpoint and
//! deleting the group takes them with it.
//!
//! The account's deliveries — each "a connection between a logical delivery source and a
//! logical delivery destination", which `CreateDelivery` creates and `DescribeDeliveries`
//! reports — live in [`ComAmazonawsCloudwatchlogsLogs20140328State::deliveries`], each one
//! a [`DeliveryRecord`], keyed by the Region it was created in and its own id
//! ([`RegionalKey`]).
//!
//! The account's storage tier policy lives here too
//! ([`ComAmazonawsCloudwatchlogsLogs20140328State::storage_tier_policy`]): it is "the
//! storage tier policy for the account", so there is one of it and no log group owns it.
//!
//! The account's delivery destinations — each "a logical object that represents the actual
//! delivery destination", which `PutDeliveryDestination` creates or updates and
//! `GetDeliveryDestination` reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::delivery_destinations`], each one a
//! [`DeliveryDestinationRecord`], keyed by the Region it was created in and the name that
//! identifies one within it ([`RegionalKey`]). A destination's
//! delivery destination policy lives on that destination
//! ([`DeliveryDestinationRecord::policy`]), because `PutDeliveryDestinationPolicy` "assigns
//! an IAM policy to the destination" and nothing but the destination names one: one
//! destination holds at most one, and removing the destination takes its policy with it.
//!
//! The account's delivery sources — each "a logical object that represents the resource that
//! is actually sending the logs", which `PutDeliverySource` creates or updates and
//! `GetDeliverySource` reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::delivery_sources`], each one a
//! [`DeliverySourceRecord`], keyed by the Region it was created in and the name that
//! identifies one within it ([`RegionalKey`]).
//!
//! The account's logical destinations — each "a cross-account destination that receives
//! subscription log events", which `PutDestination` creates or updates and
//! `DescribeDestinations` reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::destinations`], each one a
//! [`DestinationRecord`], keyed by the `destinationName` that identifies one. A destination
//! names a physical resource of another service, so it is an account-level resource and no
//! log group owns it. Its access policy lives on that destination
//! ([`DestinationRecord::access_policy`]), because `PutDestinationPolicy` creates or updates
//! "an access policy associated with an existing destination" and nothing but the destination
//! names one: one destination holds at most one, and removing the destination takes its policy
//! with it.
//!
//! The account's log anomaly detectors — "one anomaly detector in the account", which
//! `CreateLogAnomalyDetector` creates and `GetLogAnomalyDetector` reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::anomaly_detectors`], each one an
//! [`AnomalyDetectorRecord`] carrying the instant the clock owes it its next status. A
//! detector is an account-level resource rather than a log group's: it watches log groups
//! named by their ARNs and outlives none of them, so it lives beside the groups and not on
//! [`LogGroupRecord`].
//!
//! The account's integrations — each "an integration between CloudWatch Logs and another
//! service in this account", which `PutIntegration` creates and `GetIntegration` /
//! `ListIntegrations` report — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::integrations`], each one an
//! [`IntegrationRecord`] carrying the instant the clock owes it its ACTIVE status.
//!
//! The data-source associations of the account's S3 Table Integrations — each the pairing
//! `AssociateSourceToS3TableIntegration` creates and `ListSourcesForS3TableIntegration`
//! reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::s3_table_integration_sources`], each one
//! an [`S3TableIntegrationSourceRecord`] carrying the instant the clock owes its deletion.
//! An association names its integration by the ARN the associate was given rather than by a
//! record of this store: no operation of this service creates an S3 Table Integration.
//!
//! The account's lookup tables — the reference data `CreateLookupTable` uploads and
//! `GetLookupTable` reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::lookup_tables`], each one a
//! [`LookupTableRecord`], keyed by the name that identifies one ("The name must be unique
//! within your account and Region"). A lookup table enriches the queries of any log group,
//! so it is an account-level resource rather than one group's and lives beside the groups
//! and not on [`LogGroupRecord`].
//!
//! The account's scheduled queries — the queries `CreateScheduledQuery` schedules "at regular
//! intervals" and `GetScheduledQuery` reports — live in
//! [`ComAmazonawsCloudwatchlogsLogs20140328State::scheduled_queries`], each one a
//! [`ScheduledQueryRecord`], keyed by the name that identifies one ("The name must be unique
//! within your account and region"). A scheduled query names the log groups it queries as a
//! value it carries, and outlives none of them, so it is an account-level resource rather
//! than one group's and lives beside the groups and not on [`LogGroupRecord`]. Either the name
//! or the ARN identifies one to a reader
//! ([`ComAmazonawsCloudwatchlogsLogs20140328State::scheduled_query`]). The scheduled query
//! CloudWatch MANAGES for a log alarm is not here: it belongs to that alarm, so it is derived
//! from it ([`log_alarm_queries`]) rather than copied into this store.
//!
//! The vended-log delivery CONFIGURATION TEMPLATES — "the valid and default values that are
//! used when creating delivery sources, delivery destinations, and deliveries", which
//! `DescribeConfigurationTemplates` reports — are not an account's resources at all: no
//! operation creates, updates or deletes one, so they live in the catalogue
//! [`CONFIGURATION_TEMPLATE_SOURCES`] and are rendered by [`configuration_templates`]
//! rather than in a map of
//! [`ComAmazonawsCloudwatchlogsLogs20140328State`]. That is also what makes two calls
//! report the same templates.

use std::collections::BTreeMap;

/// Compiling a CloudWatch Logs Insights QL query string, which is what decides whether
/// `StartQuery` accepts one.
pub mod query_compile;

/// Which log groups a started query reads, and whether the request naming them may be
/// admitted at all.
pub mod query_admission;

/// RUNNING a started query over the stored log events, which is what `GetQueryResults`
/// reports.
pub mod insights;

pub use insights::{
    log_record_pointer, log_record_pointer_parts, LogRecordPointer, MAX_PAGE_ITEMS,
};

/// Running a `filterPattern` over a log event message, which is what `TestMetricFilter`
/// reports.
pub mod filter_pattern;

/// Metric extraction from an ingested Embedded Metric Format log event.
pub mod emf;

/// Metric extraction from an ingested log event through a log group's metric filters.
pub mod metric_filters;

/// Delivery of an ingested log event to the destinations of its log group's subscription
/// filters.
pub mod subscription_delivery;

/// Whether CloudWatch Logs can assume the role a subscription delivery is configured under,
/// which is what a put that configures one validates.
pub mod delivery_role;

/// Building another emulated service's AWS SDK client over the in-process callback, which is
/// how every call this service makes to another one is made.
pub mod callback_client;

/// Resolving the S3 bucket an export task's `destination` names against the object store.
pub mod export_destination;

/// Resolving the resource a delivery source's `resourceArn` names against the service that
/// owns it.
pub mod delivery_source_resource;

/// Resolving the data source an import task's `importSourceArn` names against the service
/// that holds it.
pub mod import_source;

/// Writing an export task's selected log events to its destination bucket, which is the work
/// the task does.
pub mod export_worker;

/// Running a started Insights query at the instant it completes, which is what fixes the
/// results that query answers from then on.
pub mod query_worker;

/// Whether the KMS key a request names may be used to encrypt the resource it names, which
/// is what the key's own key policy decides.
pub mod kms_key_grant;

/// Whether the VPC endpoint a syslog configuration ingests through is one the account has,
/// which is what Amazon EC2 — the service that owns VPC endpoints — answers.
pub mod syslog_vpc_endpoint;

/// The AWS-vended log record formats a log group's transformer parses.
pub mod log_transformer;

/// The transformation a log group's transformer applies to an ingested log event.
pub mod transformer;

/// The scheduled query CloudWatch manages on a log alarm's behalf.
pub mod log_alarm_queries;

/// The rules a scheduled query's configuration is held to, shared by the create and the update
/// that state one.
mod scheduled_query_rules;
pub use scheduled_query_rules::*;

/// The log anomaly detectors this account holds, as they are stored.
mod anomaly_detectors;
pub use anomaly_detectors::*;

/// The OpenSearch Service integrations, their S3-table sources and the scheduled queries
/// this account holds, as they are stored.
mod integration_records;
pub use integration_records::*;

/// Reading the policy documents this service's operations accept: an account policy, an
/// index policy, a subscription-filter policy and a data-protection policy.
mod policy_documents;
pub use policy_documents::*;

/// Masking the sensitive data a log group's data protection policy names, as an event is
/// ingested.
pub mod data_protection;

/// The vended-log delivery resources — a delivery, its source and its destination — as they
/// are stored.
mod deliveries;
pub use deliveries::*;

/// Which configuration template governs a delivery, and what it defaults the delivery's own
/// configuration to.
mod delivery_configuration;
pub use delivery_configuration::*;

/// What makes a document the cross-account policy a delivery destination carries.
mod delivery_destination_policy;
pub use delivery_destination_policy::*;

/// How a refusal about a PAIR of input parameters is worded, for every pair this service
/// checks across two members.
pub mod parameter_pairs;

/// Whether the CALLER's own identity-based IAM policies allow the action an operation whose
/// model declares `AccessDeniedException` asks to perform.
pub mod identity_authorization;

/// Per-service state for com-amazonaws-cloudwatchlogs-logs-20140328. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudwatchlogsLogs20140328State {
    /// Every created log group, keyed by `logGroupName`. A `BTreeMap` so
    /// `DescribeLogGroups` enumerates in log-group-name order without re-sorting.
    pub log_groups: BTreeMap<String, LogGroupRecord>,
    /// Monotonic ingestion counter. It totally orders events that share a timestamp
    /// and makes the pagination cursors and sequence tokens stable.
    pub next_ingestion_seq: u64,
    /// The account-scoped resource policies — the ones `PutResourcePolicy` creates
    /// without a `resourceArn` — keyed by `policyName`. A `BTreeMap` so
    /// `DescribeResourcePolicies` enumerates them in policy-name order. A
    /// resource-SCOPED policy is not here: it is attached to the log group its ARN
    /// names ([`LogGroupRecord::resource_policy`]), so deleting that group revokes its
    /// policy with it.
    pub account_resource_policies: BTreeMap<String, ResourcePolicyRecord>,
    /// Monotonic counter behind the resource-policy revision ids, so every update of a
    /// resource-scoped policy mints a revision no earlier update used.
    pub next_policy_revision: u64,
    /// Every saved CloudWatch Logs Insights query definition, keyed by its
    /// `queryDefinitionId` — the only thing that identifies one. A `BTreeMap` so
    /// `DescribeQueryDefinitions` enumerates them in a stable order and its pagination
    /// cursor resumes at a position that does not move when another definition is saved.
    pub query_definitions: BTreeMap<String, QueryDefinitionRecord>,
    /// Monotonic counter behind the `queryDefinitionId`s `PutQueryDefinition` mints, so
    /// every definition it creates gets an id no earlier create used — including after an
    /// earlier definition has been deleted. See [`mint_query_definition_id`].
    pub next_query_definition_seq: u64,
    /// The KMS key associated with the account's stored query-insights results — the
    /// `AssociateKmsKey` target named by the `query-result` ARN rather than by a log
    /// group ("use that key to encrypt the stored results of all future StartQuery
    /// operations in the account"). `None` until one is associated, and `None` again
    /// after `DisassociateKmsKey`.
    pub query_results_kms_key_id: Option<String>,
    /// The account-level policies of `PutAccountPolicy`, keyed by `(policyType,
    /// policyName)`: the two members `DeleteAccountPolicy` identifies one by, so policies
    /// of different types may share a name and a put under a pair already stored updates
    /// that policy in place. A `BTreeMap` so `DescribeAccountPolicies` enumerates one
    /// type's policies in policy-name order without re-sorting.
    ///
    /// Account-level policies are not attached to any log group — they "apply to all log
    /// groups in the account", including ones created later — so they live here rather
    /// than on [`LogGroupRecord`].
    pub account_policies: BTreeMap<(String, String), AccountPolicyRecord>,
    /// Every import task of this account — the resource `CreateImportTask` starts and
    /// `DescribeImportTasks` reports — keyed by `importId`. A `BTreeMap` so
    /// `DescribeImportTasks` enumerates in import-id order without re-sorting, which is
    /// also what makes its pagination cursor stable.
    pub import_tasks: BTreeMap<String, ImportTaskRecord>,
    /// Monotonic counter behind the `importId`s `CreateImportTask` mints, so every import task
    /// it creates gets an id no earlier create used — including after the task holding an id
    /// has been dropped. See [`mint_import_task_id`].
    pub next_import_task_seq: u64,
    /// Every export task of this account — the resource `CreateExportTask` starts and
    /// `DescribeExportTasks` reports — keyed by `ExportTask$taskId`. A `BTreeMap` so
    /// `DescribeExportTasks` enumerates in task-id order without re-sorting, which is also
    /// what makes its pagination cursor stable.
    pub export_tasks: BTreeMap<String, ExportTaskRecord>,
    /// Every CloudWatch Logs Insights query of this account — the one `StartQuery` starts
    /// and `DescribeQueries` reports — keyed by `QueryInfo$queryId`. A `BTreeMap` so
    /// `DescribeQueries` enumerates in query-id order without re-sorting, which is also what
    /// makes its pagination cursor stable.
    pub queries: BTreeMap<String, QueryRecord>,
    /// Monotonic counter behind the `queryId`s `StartQuery` mints, so every query it starts
    /// gets an id no earlier one used. See [`mint_query_id`].
    pub next_query_seq: u64,
    /// Monotonic counter behind the `taskId`s `CreateExportTask` mints, so every task it
    /// creates gets an id no earlier create used. See [`mint_export_task_id`].
    pub next_export_task_seq: u64,
    /// Every delivery of this account — the resource `CreateDelivery` creates and
    /// `DescribeDeliveries` reports — keyed by the Region it was created in and its
    /// `Delivery$id` ([`RegionalKey`]). A `BTreeMap` so `DescribeDeliveries` enumerates one
    /// Region's deliveries in delivery-id order without re-sorting, which is also what makes
    /// its pagination cursor stable.
    ///
    /// A delivery is an account-level resource rather than a log group's: its source and
    /// its destination are both named by their own resources, so it lives here and not on
    /// [`LogGroupRecord`].
    pub deliveries: BTreeMap<RegionalKey, DeliveryRecord>,
    /// Monotonic counter behind the `Delivery$id`s `CreateDelivery` mints, so every delivery
    /// it creates gets an id no earlier create used — including after the delivery holding an
    /// id has been deleted. See [`mint_delivery_id`].
    pub next_delivery_seq: u64,
    /// The account's storage tier policy — what `PutStorageTierPolicy` sets and
    /// `GetStorageTierPolicy` reports. `None` until a put sets one, which is why
    /// `GetStorageTierPolicy` reports [`DEFAULT_STORAGE_TIER`] with no
    /// `lastUpdatedTime`: the policy is "for the account", so there is exactly one of it
    /// and reading it before any put is not a missing resource.
    pub storage_tier_policy: Option<StorageTierPolicyRecord>,
    /// Every delivery destination of this account — the resource `PutDeliveryDestination`
    /// creates or updates and `GetDeliveryDestination` / `DescribeDeliveryDestinations`
    /// report — keyed by the Region it was created in and its `DeliveryDestination$name`
    /// ([`RegionalKey`]): the name is what identifies one within a Region ("This name must be
    /// unique for all delivery destinations in your account"). A `BTreeMap` so
    /// `DescribeDeliveryDestinations` enumerates one Region's destinations in
    /// destination-name order without re-sorting, which is also what makes its pagination
    /// cursor stable.
    ///
    /// A delivery destination is a LOGICAL object naming a resource of another service (or
    /// another log group), so it lives here rather than on [`LogGroupRecord`]: deleting the
    /// log group it points at does not delete the destination.
    pub delivery_destinations: BTreeMap<RegionalKey, DeliveryDestinationRecord>,
    /// Every delivery source of this account — the resource `PutDeliverySource` creates or
    /// updates and `GetDeliverySource` / `DescribeDeliverySources` report — keyed by the
    /// Region it was created in and its `DeliverySource$name` ([`RegionalKey`]): the name is
    /// what identifies one within a Region ("This name must be unique for all delivery
    /// sources in your account"). A `BTreeMap` so `DescribeDeliverySources` enumerates one
    /// Region's sources in source-name order without re-sorting, which is also what makes its
    /// pagination cursor stable.
    ///
    /// A delivery source is a LOGICAL object naming the resource of another service that
    /// sends the logs, so it lives here rather than on [`LogGroupRecord`]: no log group owns
    /// one, and a delivery pairs it with a destination by the name it is stored under
    /// ([`DeliveryRecord::delivery_source_name`]).
    pub delivery_sources: BTreeMap<RegionalKey, DeliverySourceRecord>,
    /// Every logical destination of this account — the resource `PutDestination` creates or
    /// updates, `DescribeDestinations` reports and `DeleteDestination` removes — keyed by
    /// `Destination$destinationName`, the one member every other destination operation
    /// identifies one by. A `BTreeMap` so `DescribeDestinations` enumerates them in the
    /// order it documents ("The results are ASCII-sorted by destination name") without
    /// re-sorting, which is also what makes its pagination cursor stable.
    ///
    /// A destination is a LOGICAL object naming a physical resource of another service, so
    /// it lives here rather than on [`LogGroupRecord`]: the log groups whose subscription
    /// filters deliver to it neither own it nor outlive it.
    pub destinations: BTreeMap<String, DestinationRecord>,
    /// Every log anomaly detector of this account — the resource `CreateLogAnomalyDetector`
    /// creates and `GetLogAnomalyDetector` reports — keyed by `anomalyDetectorArn`, the one
    /// member every other anomaly-detector operation identifies one by. A `BTreeMap` so
    /// `ListLogAnomalyDetectors` enumerates in detector-ARN order without re-sorting, which
    /// is also what makes its pagination cursor stable.
    pub anomaly_detectors: BTreeMap<String, AnomalyDetectorRecord>,
    /// Monotonic counter behind the anomaly-detector identifiers, so every create mints an
    /// ARN no earlier create used — including after the detector holding it was deleted.
    pub next_anomaly_detector_seq: u64,
    /// Every lookup table of this account — the reference data `CreateLookupTable` uploads
    /// and `GetLookupTable` reports — keyed by `lookupTableName`, which is what identifies
    /// one ("The name must be unique within your account and Region"). A `BTreeMap` so
    /// `DescribeLookupTables` enumerates in table-name order without re-sorting, which is
    /// what its `lookupTableNamePrefix` filter and its pagination cursor both read.
    pub lookup_tables: BTreeMap<String, LookupTableRecord>,
    /// Every scheduled query of this account — the one `CreateScheduledQuery` creates and
    /// `GetScheduledQuery` reports — keyed by `ScheduledQueryName`, which is what identifies
    /// one ("The name must be unique within your account and region"). A `BTreeMap` so the
    /// stored order is stable in query-name order without re-sorting.
    pub scheduled_queries: BTreeMap<String, ScheduledQueryRecord>,
    /// Every integration of this account — the resource `PutIntegration` creates,
    /// `GetIntegration` and `ListIntegrations` report and `DeleteIntegration` removes —
    /// keyed by `integrationName`, the one member every other integration operation
    /// identifies one by.
    ///
    /// An integration is an account-level resource rather than a log group's: it connects
    /// the account's logs to another service, and no log group owns it.
    pub integrations: BTreeMap<String, IntegrationRecord>,
    /// Monotonic counter behind [`IntegrationRecord::creation_seq`], so the creation order
    /// `ListIntegrations` enumerates by is total and never reused — including after the
    /// integration holding a position has been deleted.
    pub next_integration_seq: u64,
    /// Every data-source association of an S3 Table Integration — the one
    /// `AssociateSourceToS3TableIntegration` creates,
    /// `ListSourcesForS3TableIntegration` reports and
    /// `DisassociateSourceFromS3TableIntegration` removes — keyed by the
    /// `S3TableIntegrationSourceIdentifier` the associate minted for it, which is the one
    /// member every other operation of the family identifies one by.
    ///
    /// An association is account-level rather than a log group's: it pairs a data source
    /// with the integration named by an ARN, and no log group owns either. A `BTreeMap` so
    /// one pass enumerates in identifier order — which is the order the identifiers were
    /// minted in ([`mint_s3_table_integration_source_identifier`]) — without re-sorting,
    /// which is also what makes `ListSourcesForS3TableIntegration`'s pagination cursor
    /// stable.
    pub s3_table_integration_sources: BTreeMap<String, S3TableIntegrationSourceRecord>,
    /// Monotonic counter behind the `S3TableIntegrationSourceIdentifier`s an associate
    /// mints, so every association gets an identifier no earlier one used — including after
    /// the association holding an identifier has been disassociated.
    pub next_s3_table_integration_source_seq: u64,
}

impl ComAmazonawsCloudwatchlogsLogs20140328State {
    /// Take the next ingestion sequence number, advancing the counter.
    pub fn take_ingestion_seq(&mut self) -> u64 {
        let seq = self.next_ingestion_seq;
        self.next_ingestion_seq = self.next_ingestion_seq.saturating_add(1);
        seq
    }

    /// Take the next resource-policy revision number, advancing the counter.
    pub fn take_policy_revision(&mut self) -> u64 {
        let seq = self.next_policy_revision;
        self.next_policy_revision = self.next_policy_revision.saturating_add(1);
        seq
    }

    /// Whether `candidate` is a revision id this service has ever ISSUED for a
    /// resource-scoped policy.
    ///
    /// A revision id is minted from the store's monotonic counter
    /// ([`mint_revision_id`]), so the ids that exist are exactly the renderings of the
    /// counter values already taken: a value in another form, or one past the counter, is
    /// not a revision this service handed to anybody. That is what separates the two
    /// conditions a resource-policy write models — a value the service never issued is a
    /// bad PARAMETER, while a value it did issue and has since superseded is an
    /// optimistic-lock CONFLICT the caller is to re-read and retry.
    pub fn issued_revision_id(&self, candidate: &str) -> bool {
        candidate.len() == REVISION_ID_LEN
            && candidate
                .chars()
                .all(|digit| digit.is_ascii_digit() || matches!(digit, 'a'..='f'))
            && u128::from_str_radix(candidate, 16)
                .is_ok_and(|seq| seq < u128::from(self.next_policy_revision))
    }

    /// Take the next anomaly-detector sequence number, advancing the counter.
    pub fn take_anomaly_detector_seq(&mut self) -> u64 {
        let seq = self.next_anomaly_detector_seq;
        self.next_anomaly_detector_seq = self.next_anomaly_detector_seq.saturating_add(1);
        seq
    }

    /// Take the next integration sequence number, advancing the counter.
    pub fn take_integration_seq(&mut self) -> u64 {
        let seq = self.next_integration_seq;
        self.next_integration_seq = self.next_integration_seq.saturating_add(1);
        seq
    }

    /// Take the next S3-Table-Integration source sequence number, advancing the counter.
    pub fn take_s3_table_integration_source_seq(&mut self) -> u64 {
        let seq = self.next_s3_table_integration_source_seq;
        self.next_s3_table_integration_source_seq =
            self.next_s3_table_integration_source_seq.saturating_add(1);
        seq
    }

    /// Drop every data-source association whose deletion the clock has ALREADY made due, as
    /// of `now_ms`.
    ///
    /// Disassociating one is "removing query access and deleting all associated data from
    /// the integration", work that outlives the call: the association is left in
    /// [`S3_TABLE_INTEGRATION_SOURCE_STATUS_DELETE_IN_PROGRESS`] with the instant its
    /// deletion is due ([`S3TableIntegrationSourceRecord::removal_due_ms`]), and every
    /// operation of the family calls this before it reads, so what it reports is what the
    /// clock already decided — never a deletion finished because something looked.
    pub fn materialize_due_s3_table_integration_source_removals(&mut self, now_ms: i64) {
        self.s3_table_integration_sources
            .retain(|_, source| !source.removal_due(now_ms));
    }

    /// The associations of ONE S3 Table Integration, in the order their identifiers were
    /// minted, which is what `ListSourcesForS3TableIntegration` reports: "a list of data
    /// source associations for a specified S3 Table Integration".
    ///
    /// Read AFTER [`Self::materialize_due_s3_table_integration_source_removals`], so an
    /// association the clock has already deleted is not reported.
    pub fn s3_table_integration_sources_of(
        &self,
        integration_arn: &str,
    ) -> Vec<&S3TableIntegrationSourceRecord> {
        self.s3_table_integration_sources
            .values()
            .filter(|source| source.integration_arn == integration_arn)
            .collect()
    }

    /// The association that already gives `data_source` query access through the
    /// integration `integration_arn`, if the account holds one.
    ///
    /// An association IS the pairing of one data source with one integration, so a second
    /// associate of the same pair names the same association rather than a second one. An
    /// association whose data is being deleted
    /// ([`S3_TABLE_INTEGRATION_SOURCE_STATUS_DELETE_IN_PROGRESS`]) is not one of them: its
    /// query access is already removed, so associating the pair again is a new association.
    pub fn associated_s3_table_integration_source(
        &self,
        integration_arn: &str,
        data_source: &crate::ops::types::DataSource,
    ) -> Option<&S3TableIntegrationSourceRecord> {
        self.s3_table_integration_sources.values().find(|source| {
            source.integration_arn == integration_arn
                && source.removal_due_ms.is_none()
                && source.data_source.name == data_source.name
                && source.data_source.source_type == data_source.r#type
        })
    }

    /// Take the next query-definition sequence number, advancing the counter.
    pub fn take_query_definition_seq(&mut self) -> u64 {
        let seq = self.next_query_definition_seq;
        self.next_query_definition_seq = self.next_query_definition_seq.saturating_add(1);
        seq
    }

    /// Take the next delivery sequence number, advancing the counter.
    pub fn take_delivery_seq(&mut self) -> u64 {
        let seq = self.next_delivery_seq;
        self.next_delivery_seq = self.next_delivery_seq.saturating_add(1);
        seq
    }

    /// Take the next export-task sequence number, advancing the counter.
    pub fn take_export_task_seq(&mut self) -> u64 {
        let seq = self.next_export_task_seq;
        self.next_export_task_seq = self.next_export_task_seq.saturating_add(1);
        seq
    }

    /// Take the next import-task sequence number, advancing the counter.
    pub fn take_import_task_seq(&mut self) -> u64 {
        let seq = self.next_import_task_seq;
        self.next_import_task_seq = self.next_import_task_seq.saturating_add(1);
        seq
    }

    /// Take the next query sequence number, advancing the counter.
    pub fn take_query_seq(&mut self) -> u64 {
        let seq = self.next_query_seq;
        self.next_query_seq = self.next_query_seq.saturating_add(1);
        seq
    }

    /// The delivery source `region`'s endpoint holds under `name`, if any.
    ///
    /// A delivery source belongs to the Region it was created in ([`RegionalKey`]), so a name
    /// another Region holds a source under names none here.
    pub fn delivery_source(&self, region: &str, name: &str) -> Option<&DeliverySourceRecord> {
        self.delivery_sources.get(&RegionalKey::new(region, name))
    }

    /// The delivery sources `region`'s endpoint holds, in source-name order — the
    /// enumeration `DescribeDeliverySources` reports and pages.
    pub fn delivery_sources_in<'store>(
        &'store self,
        region: &str,
    ) -> impl Iterator<Item = &'store DeliverySourceRecord> {
        let region = region.to_string();
        self.delivery_sources
            .iter()
            .filter(move |(key, _record)| key.region == region)
            .map(|(_key, record)| record)
    }

    /// The delivery source — if any — that already represents `resource_arn` for `log_type`
    /// under a name OTHER than `name`.
    ///
    /// `DeliverySource$resourceArns` holds "the ARN of the Amazon Web Services resource that
    /// sends logs and is represented by this delivery source" and `DeliverySource$logType` "the
    /// type of log that the source is sending", so the pairing of the two is what a delivery
    /// addresses a sender's logs by: a second source over the same pairing would make the
    /// wiring of those logs ambiguous. A put under the name that ALREADY represents the pairing
    /// re-states its own source and is an update, which is why the source stored under `name`
    /// is never the answer.
    pub fn delivery_source_representing(
        &self,
        region: &str,
        name: &str,
        resource_arn: &str,
        log_type: &str,
    ) -> Option<&DeliverySourceRecord> {
        self.delivery_sources_in(region).find(|stored| {
            stored.name != name
                && stored.log_type == log_type
                && stored
                    .resource_arns
                    .iter()
                    .any(|stored_arn| stored_arn == resource_arn)
        })
    }

    /// The delivery destination `region`'s endpoint holds under `name`, if any. See
    /// [`Self::delivery_source`]: a destination belongs to the Region it was created in too.
    pub fn delivery_destination(
        &self,
        region: &str,
        name: &str,
    ) -> Option<&DeliveryDestinationRecord> {
        self.delivery_destinations
            .get(&RegionalKey::new(region, name))
    }

    /// The same delivery destination, to write — what assigning or removing its delivery
    /// destination policy reaches.
    pub fn delivery_destination_mut(
        &mut self,
        region: &str,
        name: &str,
    ) -> Option<&mut DeliveryDestinationRecord> {
        self.delivery_destinations
            .get_mut(&RegionalKey::new(region, name))
    }

    /// The delivery destinations `region`'s endpoint holds, in destination-name order — the
    /// enumeration `DescribeDeliveryDestinations` reports and pages.
    pub fn delivery_destinations_in<'store>(
        &'store self,
        region: &str,
    ) -> impl Iterator<Item = &'store DeliveryDestinationRecord> {
        let region = region.to_string();
        self.delivery_destinations
            .iter()
            .filter(move |(key, _record)| key.region == region)
            .map(|(_key, record)| record)
    }

    /// The delivery `region`'s endpoint holds under `id`, if any. See
    /// [`Self::delivery_source`]: a delivery belongs to the Region it was created in too.
    pub fn delivery(&self, region: &str, id: &str) -> Option<&DeliveryRecord> {
        self.deliveries.get(&RegionalKey::new(region, id))
    }

    /// The same delivery, to write — what `UpdateDeliveryConfiguration` reaches.
    pub fn delivery_mut(&mut self, region: &str, id: &str) -> Option<&mut DeliveryRecord> {
        self.deliveries.get_mut(&RegionalKey::new(region, id))
    }

    /// The deliveries `region`'s endpoint holds, in delivery-id order — the enumeration
    /// `DescribeDeliveries` reports and pages, and what answers whether a source or a
    /// destination of that Region is still in use.
    pub fn deliveries_in<'store>(
        &'store self,
        region: &str,
    ) -> impl Iterator<Item = &'store DeliveryRecord> {
        let region = region.to_string();
        self.deliveries
            .iter()
            .filter(move |(key, _record)| key.region == region)
            .map(|(_key, record)| record)
    }

    /// The tags assigned to the resource a tagging operation named, or `None` when
    /// `region`'s endpoint holds no such resource.
    ///
    /// One resource holds ONE tag set, on the resource's own record, so every operation
    /// that reads or writes tags — and every reporter of the resource itself, such as
    /// `ListLogGroups`'s `logGroupTags` filter — sees the same map.
    pub fn resource_tags(
        &self,
        region: &str,
        resource: &TaggedResource,
    ) -> Option<&BTreeMap<String, String>> {
        match resource {
            TaggedResource::LogGroup(name) => self.log_groups.get(name).map(|group| &group.tags),
            TaggedResource::Delivery(id) => {
                self.delivery(region, id).map(|delivery| &delivery.tags)
            }
            TaggedResource::Destination(name) => self
                .destinations
                .get(name)
                .map(|destination| &destination.tags),
            TaggedResource::DeliverySource(name) => {
                self.delivery_source(region, name).map(|source| &source.tags)
            }
            TaggedResource::DeliveryDestination(name) => self
                .delivery_destination(region, name)
                .map(|destination| &destination.tags),
            TaggedResource::LookupTable(name) => {
                self.lookup_tables.get(name).map(|table| &table.tags)
            }
        }
    }

    /// The delivery destination `region`'s endpoint holds under `arn`, if any.
    ///
    /// A destination is keyed by the Region it was created in and the name that identifies
    /// one there, but a delivery names its destination by ARN
    /// (`CreateDelivery$deliveryDestinationArn`), so the lookup a delivery needs is by the ARN
    /// each stored destination carries — among the destinations of the Region the delivery is
    /// being made in, which is the only Region's resources that endpoint serves.
    pub fn delivery_destination_by_arn(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&DeliveryDestinationRecord> {
        self.delivery_destinations_in(region)
            .find(|destination| destination.arn == arn)
    }

    /// The logical destination this account holds under the ARN `destination_arn`, if any.
    ///
    /// A destination is keyed by its `destinationName`, which is the only thing
    /// `PutDestinationPolicy` and `DeleteDestination` identify one by, while a subscription
    /// filter names its destination by ARN (`PutSubscriptionFilter$destinationArn`) — so the
    /// name is read off the ARN's `destination` resource segment
    /// ([`destination_name_from_arn`]) and the store is asked for it. `None` for an ARN that
    /// names no `destination` resource at all, and for one naming a destination this account
    /// does not hold — another account's destination, which this store knows nothing about.
    pub fn destination_by_arn(&self, destination_arn: &str) -> Option<&DestinationRecord> {
        self.destinations.get(destination_name_from_arn(destination_arn)?)
    }

    /// The scheduled query this account holds under `identifier`, if any.
    ///
    /// A scheduled query is keyed by its name, but every reader identifies one by "The ARN or
    /// name of the scheduled query" (`ScheduledQueryIdentifier`), so both forms resolve to the
    /// same record: the name is the store's key, and an ARN names the query its resource
    /// segment does ([`scheduled_query_name_from_identifier`]).
    pub fn scheduled_query(&self, identifier: &str) -> Option<&ScheduledQueryRecord> {
        self.scheduled_queries.get(identifier).or_else(|| {
            self.scheduled_queries
                .get(scheduled_query_name_from_identifier(identifier))
        })
    }

    /// The same scheduled query, to write. See [`Self::scheduled_query`]: both forms of
    /// `ScheduledQueryIdentifier` resolve to the one record, so a write addressed by an ARN
    /// and a write addressed by a name reach the same query.
    pub fn scheduled_query_mut(&mut self, identifier: &str) -> Option<&mut ScheduledQueryRecord> {
        let key = if self.scheduled_queries.contains_key(identifier) {
            identifier
        } else {
            scheduled_query_name_from_identifier(identifier)
        };
        self.scheduled_queries.get_mut(key)
    }

    /// Remove the scheduled query `identifier` names, returning the record that was stored
    /// under it, or `None` when the account holds none under it.
    ///
    /// Resolved exactly as the readers resolve one (see [`Self::scheduled_query`]), so a
    /// removal addressed by an ARN and a removal addressed by a name reach the same query —
    /// and the whole configuration goes with the record, because the record is where all of it
    /// (the query, the schedule, the destinations and the tags) lives.
    pub fn remove_scheduled_query(&mut self, identifier: &str) -> Option<ScheduledQueryRecord> {
        let key = if self.scheduled_queries.contains_key(identifier) {
            identifier.to_string()
        } else {
            scheduled_query_name_from_identifier(identifier).to_string()
        };
        self.scheduled_queries.remove(&key)
    }

    /// The same tag set, to write. See [`Self::resource_tags`].
    pub fn resource_tags_mut(
        &mut self,
        region: &str,
        resource: &TaggedResource,
    ) -> Option<&mut BTreeMap<String, String>> {
        match resource {
            TaggedResource::LogGroup(name) => {
                self.log_groups.get_mut(name).map(|group| &mut group.tags)
            }
            TaggedResource::Delivery(id) => self
                .delivery_mut(region, id)
                .map(|delivery| &mut delivery.tags),
            TaggedResource::Destination(name) => self
                .destinations
                .get_mut(name)
                .map(|destination| &mut destination.tags),
            TaggedResource::DeliverySource(name) => self
                .delivery_sources
                .get_mut(&RegionalKey::new(region, name))
                .map(|source| &mut source.tags),
            TaggedResource::DeliveryDestination(name) => self
                .delivery_destination_mut(region, name)
                .map(|destination| &mut destination.tags),
            TaggedResource::LookupTable(name) => self
                .lookup_tables
                .get_mut(name)
                .map(|table| &mut table.tags),
        }
    }

    /// Whether an ingested log event of `group` has the metrics its Embedded Metric Format
    /// document declares extracted into CloudWatch metric data.
    ///
    /// "By default, EMF metric creation is enabled for all log groups. You can use metric
    /// extraction policies to disable EMF metric creation for your entire account or specific
    /// log groups" — and a `METRIC_EXTRACTION_POLICY` document is accepted only when it says
    /// the format is disabled ([`check_account_policy_document`]), so extraction is on for
    /// every group no such policy SELECTS ([`account_policy_selects_log_group`]) and off for
    /// the groups one selects.
    pub fn emf_extraction_enabled_for(&self, group: &LogGroupRecord) -> bool {
        !self
            .account_policies
            .values()
            .filter(|policy| policy.policy_type == POLICY_TYPE_METRIC_EXTRACTION)
            .any(|policy| account_policy_selects_log_group(policy, group))
    }

    /// Materialise every CloudWatch Logs Insights query status change the clock has ALREADY
    /// made due, as of `now_ms`.
    ///
    /// A query runs asynchronously — `StartQuery` hands back a `queryId` and the results are
    /// fetched later — so the status it is in is owed by the clock rather than by a call
    /// ([`QueryRecord::running_due_ms`], [`QueryRecord::complete_due_ms`]). A reporting
    /// operation calls this before it reads, so what it reports is what the clock already
    /// decided — never a status moved on because something was polled.
    pub fn materialize_due_query_transitions(&mut self, now_ms: i64) {
        for query in self.queries.values_mut() {
            query.materialize_due_transitions(now_ms);
        }
    }

    /// How many queries of this account are still in flight, which is what
    /// [`MAX_CONCURRENT_QUERIES`] bounds.
    ///
    /// Read AFTER [`Self::materialize_due_query_transitions`], so a query the clock has
    /// already completed does not hold a slot.
    pub fn active_queries(&self) -> usize {
        self.queries.values().filter(|query| query.is_active()).count()
    }

    /// Drop the oldest finished queries the store holds beyond [`MAX_QUERIES`], so repeated
    /// starts cannot grow it without bound. An ACTIVE query is never dropped: it has not run
    /// yet.
    pub fn enforce_query_ceiling(&mut self) {
        while self.queries.len() > MAX_QUERIES {
            let Some(oldest) = self
                .queries
                .values()
                .filter(|query| !query.is_active())
                .min_by_key(|query| (query.create_time_ms, query.query_id.clone()))
                .map(|query| query.query_id.clone())
            else {
                return;
            };
            self.queries.remove(&oldest);
        }
    }

    /// Materialise every import-task status change the clock has ALREADY made due, as of
    /// `now_ms`.
    ///
    /// An import runs asynchronously, so its terminal status is owed by the clock rather
    /// than by a call: the change was due at [`ImportTaskRecord::completion_due_ms`] even
    /// if nobody asked. A reporting operation calls this before it reads, so what it
    /// reports is what the clock already decided — never a status moved on because
    /// something was polled.
    pub fn materialize_due_import_transitions(&mut self, now_ms: i64) {
        for task in self.import_tasks.values_mut() {
            task.materialize_due_transitions(now_ms);
        }
    }

    /// How many import tasks of this account are still running, which is what
    /// [`MAX_ACTIVE_IMPORT_TASKS`] bounds ("there can be no more than 3 active imports per
    /// account at a given time").
    ///
    /// Read AFTER [`Self::materialize_due_import_transitions`], so a task the clock has
    /// already completed does not hold a slot.
    pub fn active_import_tasks(&self) -> usize {
        self.import_tasks
            .values()
            .filter(|task| !task.is_terminal())
            .count()
    }

    /// Drop the oldest finished import tasks the store holds beyond [`MAX_IMPORT_TASKS`], so
    /// repeated creates cannot grow it without bound. A task that is still running is never
    /// dropped: its import has not finished.
    pub fn enforce_import_task_ceiling(&mut self) {
        while self.import_tasks.len() > MAX_IMPORT_TASKS {
            let Some(oldest) = self
                .import_tasks
                .values()
                .filter(|task| task.is_terminal())
                .min_by_key(|task| (task.creation_time_ms, task.import_id.clone()))
                .map(|task| task.import_id.clone())
            else {
                return;
            };
            self.import_tasks.remove(&oldest);
        }
    }

    /// Materialise every export-task status change the clock has ALREADY made due, as of
    /// `now_ms`.
    ///
    /// `CreateExportTask` "is an asynchronous call", so an export task's statuses are owed by
    /// the clock rather than by a call: each change was due at the instant the record stores
    /// for it ([`ExportTaskRecord::running_due_ms`], [`ExportTaskRecord::completion_due_ms`])
    /// even if nobody asked. A reporting operation calls this before it reads, so what it
    /// reports is what the clock already decided — never a status moved on because something
    /// was polled.
    pub fn materialize_due_export_transitions(&mut self, now_ms: i64) {
        for task in self.export_tasks.values_mut() {
            task.materialize_due_transitions(now_ms);
        }
    }

    /// Whether this account already has an export task in one of the two statuses
    /// `CreateExportTask` counts as active ("Each account can only have one active (RUNNING
    /// or PENDING) export task at a time"). The slot is the account's, so holding it is all
    /// there is to know about it: the refusal names nothing of the task that holds it.
    ///
    /// Read AFTER [`Self::materialize_due_export_transitions`], so a task the clock has
    /// already completed does not count as active.
    pub fn has_active_export_task(&self) -> bool {
        self.export_tasks.values().any(ExportTaskRecord::is_active)
    }

    /// Drop the oldest export tasks the store holds beyond [`MAX_EXPORT_TASKS`], so repeated
    /// creates cannot grow it without bound. An ACTIVE task is never dropped: it is still
    /// running, and only one of those can exist at a time.
    pub fn enforce_export_task_ceiling(&mut self) {
        while self.export_tasks.len() > MAX_EXPORT_TASKS {
            let Some(oldest) = self
                .export_tasks
                .values()
                .filter(|task| !task.is_active())
                .min_by_key(|task| (task.creation_time_ms, task.task_id.clone()))
                .map(|task| task.task_id.clone())
            else {
                return;
            };
            self.export_tasks.remove(&oldest);
        }
    }

    /// Materialise every anomaly-detector status change the clock has ALREADY made due, as
    /// of `now_ms`.
    ///
    /// A detector initializes and trains on its own, so the status it is in is owed by the
    /// clock rather than by a call ([`AnomalyDetectorRecord::status_due_ms`]). A reporting
    /// operation calls this before it reads, so what it reports is what the clock already
    /// decided — never a status moved on because something was polled.
    pub fn materialize_due_anomaly_detector_transitions(&mut self, now_ms: i64) {
        for detector in self.anomaly_detectors.values_mut() {
            detector.materialize_due_transitions(now_ms);
        }
    }

    /// How many bytes of lookup-table CSV content the account's store currently holds —
    /// what [`MAX_LOOKUP_TABLE_BYTES_TOTAL`] bounds. DERIVED by summing the stored bodies,
    /// so it cannot drift from what the store actually holds.
    pub fn lookup_table_bytes(&self) -> usize {
        self.lookup_tables
            .values()
            .map(|table| table.table_body.len())
            .sum()
    }

    /// Materialise every integration status change the clock has ALREADY made due, as of
    /// `now_ms`.
    ///
    /// An integration provisions on its own — "it takes a few minutes to complete" — so the
    /// status it is in is owed by the clock rather than by a call
    /// ([`IntegrationRecord::status_due_ms`]). A reporting operation calls this before it
    /// reads, so what it reports is what the clock already decided — never a status moved
    /// on because something was polled.
    pub fn materialize_due_integration_transitions(&mut self, now_ms: i64) {
        for integration in self.integrations.values_mut() {
            integration.materialize_due_transitions(now_ms);
        }
    }

    /// The account's integrations in the order they were created
    /// ([`IntegrationRecord::creation_seq`]), which is what `ListIntegrations` reports.
    ///
    /// AWS documents no order for that operation, so this one is the emulator's: creation
    /// order is stable across calls and does not move when another integration is created
    /// or deleted, which name order would not be.
    pub fn integrations_in_creation_order(&self) -> Vec<&IntegrationRecord> {
        let mut integrations: Vec<&IntegrationRecord> = self.integrations.values().collect();
        integrations.sort_by_key(|integration| integration.creation_seq);
        integrations
    }
}

/// One resource policy — "a policy enabling one or more entities to put logs to a log
/// group in this account" — as it is stored.
#[derive(Debug, Clone)]
pub struct ResourcePolicyRecord {
    /// `ResourcePolicy$policyName` — "The name of the resource policy." The name is the
    /// whole identity of an account-scoped policy, so one always carries it; a
    /// resource-scoped policy is identified by the resource it is attached to, so its
    /// name is whatever the request supplied and `None` when it supplied none.
    pub policy_name: Option<String>,
    pub policy_document: String,
    pub last_updated_time_ms: i64,
    /// The ARN this policy is attached to, for a resource-scoped policy; `None` for an
    /// account-scoped one (`ResourcePolicy$resourceArn` is "only populated for
    /// resource-scoped policies").
    pub resource_arn: Option<String>,
    /// The revision this policy is at, for a resource-scoped policy; `None` for an
    /// account-scoped one (`ResourcePolicy$revisionId` is "only populated for
    /// resource-scoped policies").
    pub revision_id: Option<String>,
}

impl ResourcePolicyRecord {
    /// This policy's `policyScope`: `RESOURCE` when it is attached to a resource,
    /// `ACCOUNT` when it is attached to none.
    pub fn policy_scope(&self) -> &'static str {
        if self.resource_arn.is_some() {
            POLICY_SCOPE_RESOURCE
        } else {
            POLICY_SCOPE_ACCOUNT
        }
    }
}

/// One account-level policy — what `PutAccountPolicy` stores — as it is stored.
///
/// The document is kept VERBATIM, exactly the bytes the request carried, for the same
/// reason as [`DataProtectionPolicyRecord::policy_document`]: AWS reports it back through
/// `DescribeAccountPolicies` unchanged.
#[derive(Debug, Clone)]
pub struct AccountPolicyRecord {
    /// `AccountPolicy$policyName` — "The name of the account policy."
    pub policy_name: String,
    /// `AccountPolicy$policyDocument` — "The policy document for this account policy."
    pub policy_document: String,
    /// `AccountPolicy$policyType` — "The type of policy for this account policy."
    pub policy_type: String,
    /// `AccountPolicy$scope` — "The scope of the account policy."
    pub scope: String,
    /// `AccountPolicy$selectionCriteria` — "The log group selection criteria that is used
    /// for this policy." `None` for a policy that applies to all log groups.
    pub selection_criteria: Option<String>,
    /// `AccountPolicy$lastUpdatedTime` — "The date and time that this policy was most
    /// recently updated."
    pub last_updated_time_ms: i64,
}

impl AccountPolicyRecord {
    /// This policy as the model's `AccountPolicy` structure, which is what both
    /// `PutAccountPolicy` and `DescribeAccountPolicies` report, so one record renders the
    /// same way through either.
    ///
    /// `accountId` is "The Amazon Web Services account ID that the policy applies to" —
    /// this emulator's own account, because an account-level policy applies to the account
    /// that created it.
    pub fn reported(&self) -> crate::ops::types::AccountPolicy {
        crate::ops::types::AccountPolicy {
            policy_name: Some(self.policy_name.clone()),
            policy_document: Some(self.policy_document.clone()),
            last_updated_time: Some(self.last_updated_time_ms),
            policy_type: Some(self.policy_type.clone()),
            scope: Some(self.scope.clone()),
            selection_criteria: self.selection_criteria.clone(),
            account_id: Some(::engine::emulator_account_id().to_string()),
        }
    }
}

/// One import task — "An import job to move data from CloudTrail Event Data Store to
/// CloudWatch" — as it is stored. Every member of the model's `Import` shape is reported
/// from this record, so `DescribeImportTasks` answers from state alone.
#[derive(Debug, Clone)]
pub struct ImportTaskRecord {
    /// `Import$importId` — "The unique identifier of the import task."
    pub import_id: String,
    /// `Import$importSourceArn` — "The ARN of the CloudTrail Lake Event Data Store being
    /// imported from."
    pub import_source_arn: String,
    /// `CreateImportTask$importRoleArn` — "The ARN of the IAM role that grants CloudWatch
    /// Logs permission to import". No `Import` member reports it, but the task is the only
    /// record of the role the import runs under, so it is stored with the task, by the
    /// `CreateImportTask` that starts the import.
    pub import_role_arn: String,
    /// `Import$importDestinationArn` — "The ARN of the managed CloudWatch Logs log group
    /// where the events are being imported to."
    pub import_destination_arn: String,
    /// `Import$importStatus`, one of the [`IMPORT_STATUS_IN_PROGRESS`] family.
    pub import_status: String,
    /// `ImportFilter$startEventTime`, when the create named one.
    pub start_event_time_ms: Option<i64>,
    /// `ImportFilter$endEventTime`, when the create named one.
    pub end_event_time_ms: Option<i64>,
    /// `ImportStatistics$bytesImported` — "The total number of bytes that have been
    /// imported to the managed log group."
    pub bytes_imported: i64,
    /// `Import$creationTime`, in epoch milliseconds.
    pub creation_time_ms: i64,
    /// `Import$lastUpdatedTime`, in epoch milliseconds.
    pub last_updated_time_ms: i64,
    /// The instant this task's asynchronous import is due to reach `COMPLETED`, stored
    /// rather than waited for: a reporting operation materialises the transition once the
    /// clock is past it ([`materialize_due_import_transitions`]). `None` once the task is
    /// in a terminal status, which owes no further transition.
    ///
    /// [`materialize_due_import_transitions`]:
    ///     ComAmazonawsCloudwatchlogsLogs20140328State::materialize_due_import_transitions
    pub completion_due_ms: Option<i64>,
    /// `Import$errorMessage` — "Error message related to any failed imports". Only a
    /// FAILED task carries one.
    pub error_message: Option<String>,
}

impl ImportTaskRecord {
    /// A brand-new import task: running, nothing imported yet, and due to complete
    /// [`IMPORT_COMPLETION_DELAY_MS`] after it was created. This is what
    /// `CreateImportTask` stores for the import it starts.
    pub fn new(
        import_id: &str,
        import_source_arn: &str,
        import_role_arn: &str,
        import_destination_arn: &str,
        import_filter: Option<&crate::ops::types::ImportFilter>,
        creation_time_ms: i64,
    ) -> Self {
        Self {
            import_id: import_id.to_string(),
            import_source_arn: import_source_arn.to_string(),
            import_role_arn: import_role_arn.to_string(),
            import_destination_arn: import_destination_arn.to_string(),
            import_status: IMPORT_STATUS_IN_PROGRESS.to_string(),
            start_event_time_ms: import_filter.and_then(|filter| filter.start_event_time),
            end_event_time_ms: import_filter.and_then(|filter| filter.end_event_time),
            bytes_imported: 0,
            creation_time_ms,
            last_updated_time_ms: creation_time_ms,
            completion_due_ms: Some(creation_time_ms.saturating_add(IMPORT_COMPLETION_DELAY_MS)),
            error_message: None,
        }
    }

    /// Whether this task has reached a status it can never leave — anything but
    /// [`IMPORT_STATUS_IN_PROGRESS`], the one status the model's `ImportStatus` enum
    /// describes as still running. This is the condition `CancelImportTask` refuses on
    /// ("Cancels an ACTIVE import task"), and the one that tells this record it owes no
    /// further clock transition.
    pub fn is_terminal(&self) -> bool {
        self.import_status != IMPORT_STATUS_IN_PROGRESS
    }

    /// This task's `importFilter`, or `None` when the create bounded the event time at
    /// neither end — `ImportFilter`'s two members are both optional, so a task created
    /// without either carries no filter to report.
    pub fn import_filter(&self) -> Option<crate::ops::types::ImportFilter> {
        let filter = crate::ops::types::ImportFilter {
            start_event_time: self.start_event_time_ms,
            end_event_time: self.end_event_time_ms,
        };
        (filter.start_event_time.is_some() || filter.end_event_time.is_some()).then_some(filter)
    }

    /// This task's `importStatistics`: what the import has moved so far, counted in state
    /// rather than reported as progress nothing made.
    pub fn import_statistics(&self) -> crate::ops::types::ImportStatistics {
        crate::ops::types::ImportStatistics {
            bytes_imported: Some(self.bytes_imported),
        }
    }

    /// Materialise the status change the clock owes this task: a running import whose
    /// completion instant has passed is `COMPLETED`, updated AT that instant and not at
    /// the time something happened to look.
    pub fn materialize_due_transitions(&mut self, now_ms: i64) {
        if self.is_terminal() {
            return;
        }
        let Some(due) = self.completion_due_ms else {
            return;
        };
        if due <= now_ms {
            self.import_status = IMPORT_STATUS_COMPLETED.to_string();
            self.last_updated_time_ms = due;
            self.completion_due_ms = None;
        }
    }
}

/// One CloudWatch Logs Insights query — what `StartQuery` starts and `DescribeQueries`
/// reports — as it is stored. Every member of the model's `QueryInfo` shape is reported from
/// this record, so `DescribeQueries` answers from state alone.
#[derive(Debug, Clone)]
pub struct QueryRecord {
    /// `QueryInfo$queryId` — "The unique ID number of this query." Also the key the store
    /// enumerates by.
    pub query_id: String,
    /// `QueryInfo$queryLanguage` — "The query language used for this query",
    /// [`DEFAULT_QUERY_LANGUAGE`] when the start named none.
    pub query_language: String,
    /// `QueryInfo$queryString` — "The query string used in this query."
    pub query_string: String,
    /// `QueryInfo$logGroupName` — "The name of the log group scanned by this query", for a
    /// query that named exactly one. A query over several log groups, or one whose `SOURCE`
    /// command selects them, carries the whole list in [`Self::log_group_names`] and reports
    /// no single name.
    pub log_group_name: Option<String>,
    /// Every log group this query reads, resolved to log group NAMES from whichever member
    /// the start named them by. Empty for a query whose `SOURCE` command selects its own.
    pub log_group_names: Vec<String>,
    /// `StartQuery$startTime` — the beginning of the time range queried, in epoch SECONDS,
    /// the unit that member documents ("Specified as epoch time, the number of seconds since
    /// January 1, 1970, 00:00:00 UTC").
    pub start_time_secs: i64,
    /// `StartQuery$endTime` — the end of that range, in epoch seconds. See
    /// [`Self::start_time_secs`].
    pub end_time_secs: i64,
    /// `StartQuery$limit` — "The maximum number of log events to return from the query",
    /// when the start named one.
    pub limit: Option<i32>,
    /// `QueryInfo$status`, one of the [`QUERY_STATUS_SCHEDULED`] family.
    pub status: String,
    /// `QueryInfo$createTime` — "The date and time that this query was created", in epoch
    /// milliseconds.
    pub create_time_ms: i64,
    /// The instant this query is due to start running, stored rather than waited for: a
    /// reporting operation materialises the transition once the clock is past it
    /// ([`materialize_due_query_transitions`]). `None` once it has left `Scheduled`.
    ///
    /// [`materialize_due_query_transitions`]:
    ///     ComAmazonawsCloudwatchlogsLogs20140328State::materialize_due_query_transitions
    pub running_due_ms: Option<i64>,
    /// The instant this query is due to reach `Complete`, on the same terms as
    /// [`Self::running_due_ms`]. `None` once it is in a terminal status, which owes no
    /// further transition.
    pub complete_due_ms: Option<i64>,
    /// What this query's RUN produced, once it has run: the rows and the counts over the run
    /// ([`insights::Execution`]), `None` for a query that has not produced them.
    ///
    /// "Returns the results FROM the specified query" (`GetQueryResults`): a query that has
    /// run holds its own results, so what it answers is this snapshot and not a fresh run over
    /// whatever the log groups hold now — a group deleted, or an event ingested, after the
    /// query ran changes neither the rows nor the statistics it reports.
    pub snapshot: Option<insights::Execution>,
}

impl QueryRecord {
    /// A brand-new query: accepted, not running yet, and due to run
    /// [`QUERY_RUNNING_DELAY_MS`] after it was started and to complete
    /// [`QUERY_COMPLETION_DELAY_MS`] after that.
    pub fn new(
        query_id: &str,
        query_language: &str,
        query_string: &str,
        log_group_names: Vec<String>,
        start_time_secs: i64,
        end_time_secs: i64,
        create_time_ms: i64,
    ) -> Self {
        let running_due_ms = create_time_ms.saturating_add(QUERY_RUNNING_DELAY_MS);
        Self {
            query_id: query_id.to_string(),
            query_language: query_language.to_string(),
            query_string: query_string.to_string(),
            // "The name of the log group scanned by this query" is one name, so it is
            // reported only for a query that reads exactly one log group.
            log_group_name: match log_group_names.as_slice() {
                [only] => Some(only.clone()),
                _ => None,
            },
            log_group_names,
            start_time_secs,
            end_time_secs,
            limit: None,
            status: QUERY_STATUS_SCHEDULED.to_string(),
            create_time_ms,
            running_due_ms: Some(running_due_ms),
            complete_due_ms: Some(running_due_ms.saturating_add(QUERY_COMPLETION_DELAY_MS)),
            // A query that has not run has produced no results to report.
            snapshot: None,
        }
    }

    /// Record what this query's run produced, which is the snapshot it answers from then on
    /// ([`Self::snapshot`]).
    ///
    /// The FIRST snapshot stands: a query RUNS once, so what it produced is fixed at that run
    /// and a later execution of the same stored query replaces nothing.
    pub fn record_snapshot(&mut self, execution: insights::Execution) {
        if self.snapshot.is_none() {
            self.snapshot = Some(execution);
        }
    }

    /// Whether this query is still one of the statuses a concurrent-query quota counts: a
    /// query that has finished, failed or been cancelled holds no slot.
    pub fn is_active(&self) -> bool {
        self.status == QUERY_STATUS_SCHEDULED || self.status == QUERY_STATUS_RUNNING
    }

    /// Stop this query: it reaches [`QUERY_STATUS_CANCELLED`] and owes the clock no further
    /// transition, so the completion it was armed for is DISCARDED rather than materialised
    /// later over the cancellation. Only a query that [`Self::is_active`] has an execution
    /// left to stop.
    pub fn cancel(&mut self) {
        self.status = QUERY_STATUS_CANCELLED.to_string();
        self.running_due_ms = None;
        self.complete_due_ms = None;
    }

    /// Materialise the status changes the clock owes this query: a scheduled query whose
    /// start instant has passed is `Running`, and a running one whose completion instant has
    /// passed is `Complete` — each decided AT that instant and not at the time something
    /// happened to look.
    pub fn materialize_due_transitions(&mut self, now_ms: i64) {
        if let Some(due) = self.running_due_ms {
            if due <= now_ms {
                self.status = QUERY_STATUS_RUNNING.to_string();
                self.running_due_ms = None;
            }
        }
        if self.running_due_ms.is_some() {
            return;
        }
        if let Some(due) = self.complete_due_ms {
            if due <= now_ms {
                self.status = QUERY_STATUS_COMPLETE.to_string();
                self.complete_due_ms = None;
            }
        }
    }

    /// This query as the model's `QueryInfo` structure, which is what `DescribeQueries`
    /// reports.
    pub fn reported(&self) -> crate::ops::types::QueryInfo {
        crate::ops::types::QueryInfo {
            query_language: Some(self.query_language.clone()),
            query_id: Some(self.query_id.clone()),
            query_string: Some(self.query_string.clone()),
            status: Some(self.status.clone()),
            create_time: Some(self.create_time_ms),
            log_group_name: self.log_group_name.clone(),
            // "The duration in milliseconds that the query took to execute", "The total
            // number of bytes scanned by the query" and "The ARN of the user who ran the
            // query" are facts about an EXECUTION and an identity this emulator does not
            // hold: nothing it stores can report any of them, so none is reported rather
            // than filled with a number nothing measured.
            query_duration: None,
            bytes_scanned: None,
            user_identity: None,
        }
    }
}

/// A query that has been accepted but has not started running, from the model's
/// `QueryStatus` enum (`Scheduled` | `Running` | `Complete` | `Failed` | `Cancelled` |
/// `Timeout` | `Unknown`). `StartQuery` hands back a `queryId` and the results are fetched
/// later, so an accepted query starts here.
pub const QUERY_STATUS_SCHEDULED: &str = "Scheduled";

/// A query that is running, from the model's `QueryStatus` enum.
pub const QUERY_STATUS_RUNNING: &str = "Running";

/// A query that finished running, from the model's `QueryStatus` enum.
pub const QUERY_STATUS_COMPLETE: &str = "Complete";

/// A query a caller stopped, from the model's `QueryStatus` enum.
pub const QUERY_STATUS_CANCELLED: &str = "Cancelled";

/// A query whose EXECUTION produced no results, from the model's `QueryStatus` enum: the
/// status `GetQueryResults` reports for a query string this service cannot run over the
/// events it holds ([`insights::ExecutionFailure`]).
pub const QUERY_STATUS_FAILED: &str = "Failed";

/// How long after it is started a query moves `Scheduled` -> `Running`, and how long after
/// that it reaches `Complete`.
///
/// AWS documents no duration for a query, only the timeout it cannot exceed ("Queries time
/// out after 60 minutes of runtime"), so these spans are THE EMULATOR'S OWN CHOICE: each is
/// deliberately not zero, so a caller that reads the query straight after starting it still
/// observes it scheduled. The instants both fall due at are stored on the record and never
/// slept on.
pub const QUERY_RUNNING_DELAY_MS: i64 = 250;

/// See [`QUERY_RUNNING_DELAY_MS`]: the further span a running query takes to complete.
pub const QUERY_COMPLETION_DELAY_MS: i64 = 250;

/// How many queries one account may have in flight: "You can have up to 100 concurrent
/// CloudWatch Logs insights queries, including queries that have been added to dashboards"
/// (`StartQuery`).
pub const MAX_CONCURRENT_QUERIES: usize = 100;

/// The most log groups one query may read: "You can include up to 50 log groups"
/// (`StartQuery$logGroupNames`, `StartQuery$logGroupIdentifiers`).
pub const MAX_QUERY_LOG_GROUPS: usize = 50;

/// THE EMULATOR'S OWN CEILING on the queries one account's store holds, not a quota cited
/// from AWS: `DescribeQueries` reports "queries that are scheduled, running, or have been run
/// recently", so a finished query stays readable for a while — and the oldest finished ones
/// are dropped once the store holds this many rather than growing without bound on request.
pub const MAX_QUERIES: usize = 1_000;

/// The largest page `DescribeQueries` hands back, which is also the page a request that
/// names no `maxResults` gets: `maxResults` "limits the number of returned queries to the
/// specified number" and documents no default, and the member's own
/// `DescribeQueriesMaxResults` range (`range 1..1000`) is the most the API admits in one
/// page.
pub const MAX_QUERIES_PER_PAGE: i32 = 1_000;

/// One export task — "an export task so that you can efficiently export data from a log
/// group to an Amazon S3 bucket" — as it is stored. Every member of the model's `ExportTask`
/// shape is reported from this record, so `DescribeExportTasks` answers from state alone.
#[derive(Debug, Clone)]
pub struct ExportTaskRecord {
    /// `ExportTask$taskId` — "The ID of the export task." Also the key the store enumerates
    /// by.
    pub task_id: String,
    /// `ExportTask$taskName` — "The name of the export task", when the create named one.
    pub task_name: Option<String>,
    /// `ExportTask$logGroupName` — "The name of the log group from which logs data was
    /// exported."
    pub log_group_name: String,
    /// `ExportTask$from` — "Events with a timestamp before this time are not exported."
    pub from_ms: i64,
    /// `ExportTask$to` — "Events with a timestamp later than this time are not exported."
    pub to_ms: i64,
    /// `ExportTask$destination` — "The name of the S3 bucket to which the log data was
    /// exported."
    pub destination: String,
    /// `ExportTask$destinationPrefix` — "The prefix that was used as the start of Amazon S3
    /// key for every object exported", [`DEFAULT_EXPORT_DESTINATION_PREFIX`] when the create
    /// named none.
    pub destination_prefix: String,
    /// `CreateExportTask$logStreamNamePrefix` — "Export only log streams that match the
    /// provided prefix." No `ExportTask` member reports it, but the task is the only record
    /// of which streams it exports, so it is stored with the task.
    pub log_stream_name_prefix: Option<String>,
    /// `ExportTaskStatus$code`, one of the [`EXPORT_TASK_STATUS_PENDING`] family.
    pub status_code: String,
    /// `ExportTaskStatus$message` — "The status message related to the status code" — the
    /// message of the status this task is IN, stamped by the transition that put it there:
    /// the fixed text of the terminal class a task settles at
    /// ([`EXPORT_TASK_CANCELLED_STATUS_MESSAGE`], [`EXPORT_TASK_COMPLETED_STATUS_MESSAGE`]),
    /// or the reason an export that stopped on an error ended. `None` while the export is
    /// still under way: there is nothing to say about a task that has not reached an outcome
    /// yet.
    pub status_message: Option<String>,
    /// `ExportTaskExecutionInfo$creationTime`, in epoch milliseconds.
    pub creation_time_ms: i64,
    /// `ExportTaskExecutionInfo$completionTime`, in epoch milliseconds — set only once the
    /// task has reached a terminal status.
    pub completion_time_ms: Option<i64>,
    /// The instant this task's export is due to start moving data, stored rather than waited
    /// for: a reporting operation materialises the transition once the clock is past it
    /// ([`materialize_due_export_transitions`]). `None` once the task has left `PENDING`.
    ///
    /// [`materialize_due_export_transitions`]:
    ///     ComAmazonawsCloudwatchlogsLogs20140328State::materialize_due_export_transitions
    pub running_due_ms: Option<i64>,
    /// The instant this task's export is due to reach `COMPLETED`, on the same terms as
    /// [`Self::running_due_ms`]. `None` once the task is in a terminal status, which owes no
    /// further transition.
    pub completion_due_ms: Option<i64>,
    /// Whether the export's OWN work has finished — the selected events are in the
    /// destination bucket ([`export_worker`]).
    ///
    /// `COMPLETED` is the service's statement that the log data has been delivered, so the
    /// clock alone cannot put a task in it: a task reaches it only once the export it was
    /// created for has run ([`Self::record_export_written`]). Until then the export is still
    /// moving data, which is what `RUNNING` says — and what keeps the account's one active
    /// export slot held.
    ///
    /// [`export_worker`]: crate::state::export_worker
    pub export_written: bool,
}

impl ExportTaskRecord {
    /// A brand-new export task: accepted, not started yet, and due to start
    /// [`EXPORT_TASK_RUNNING_DELAY_MS`] after it was created and to complete
    /// [`EXPORT_TASK_COMPLETION_DELAY_MS`] after that.
    ///
    /// The optional members a create may name — `taskName`, `destinationPrefix`,
    /// `logStreamNamePrefix` — are taken here because every one of them is a property of the
    /// export the task IS.
    pub fn new(
        task_id: &str,
        log_group_name: &str,
        from_ms: i64,
        to_ms: i64,
        destination: &str,
        creation_time_ms: i64,
    ) -> Self {
        let running_due_ms = creation_time_ms.saturating_add(EXPORT_TASK_RUNNING_DELAY_MS);
        Self {
            task_id: task_id.to_string(),
            task_name: None,
            log_group_name: log_group_name.to_string(),
            from_ms,
            to_ms,
            destination: destination.to_string(),
            destination_prefix: DEFAULT_EXPORT_DESTINATION_PREFIX.to_string(),
            log_stream_name_prefix: None,
            status_code: EXPORT_TASK_STATUS_PENDING.to_string(),
            status_message: None,
            creation_time_ms,
            completion_time_ms: None,
            running_due_ms: Some(running_due_ms),
            completion_due_ms: Some(
                running_due_ms.saturating_add(EXPORT_TASK_COMPLETION_DELAY_MS),
            ),
            export_written: false,
        }
    }

    /// Record that the export's own work finished at `now_ms`: its objects are in the
    /// destination bucket, so the task may now reach `COMPLETED`.
    ///
    /// The completion the clock owes is never pulled EARLIER by this — an export that
    /// delivered its data still takes the span this emulator gives the transition — and it is
    /// pushed out when the data landed after that instant, so the instant the task reports as
    /// its completion is never before the instant the data was there.
    pub fn record_export_written(&mut self, now_ms: i64) {
        self.export_written = true;
        if let Some(due) = self.completion_due_ms {
            self.completion_due_ms = Some(due.max(now_ms));
        }
    }

    /// Whether this task is one of the two statuses `CreateExportTask` counts as active:
    /// "Each account can only have one active (RUNNING or PENDING) export task at a time."
    pub fn is_active(&self) -> bool {
        self.status_code == EXPORT_TASK_STATUS_PENDING
            || self.status_code == EXPORT_TASK_STATUS_RUNNING
    }

    /// Cancel this export task: it reaches [`EXPORT_TASK_STATUS_CANCELLED`] at `now_ms` and
    /// owes the clock no further transition, so the start and the completion it was armed for
    /// are DISCARDED rather than materialised later over the cancellation. The status message
    /// that status carries is stamped with it, the way every terminal status an export task
    /// reaches carries its own. Only a task that [`Self::is_active`] has an export left to
    /// cancel — "The task must be in the PENDING or RUNNING state."
    pub fn cancel(&mut self, now_ms: i64) {
        self.status_code = EXPORT_TASK_STATUS_CANCELLED.to_string();
        self.status_message = Some(EXPORT_TASK_CANCELLED_STATUS_MESSAGE.to_string());
        self.completion_time_ms = Some(now_ms);
        self.running_due_ms = None;
        self.completion_due_ms = None;
    }

    /// Materialise the status changes the clock owes this task: a pending export whose start
    /// instant has passed is `RUNNING`, and a running one whose completion instant has passed
    /// AND whose data is delivered is `COMPLETED` — each stamped AT that instant and not at
    /// the time something happened to look. A task the clock finishes carries the status
    /// message that terminal status carries, the way every status an export task settles at
    /// does.
    pub fn materialize_due_transitions(&mut self, now_ms: i64) {
        if let Some(due) = self.running_due_ms {
            if due <= now_ms {
                self.status_code = EXPORT_TASK_STATUS_RUNNING.to_string();
                self.running_due_ms = None;
            }
        }
        if self.running_due_ms.is_some() {
            return;
        }
        // `COMPLETED` says the export delivered the log data, so the clock completes a task
        // only once the export itself has ([`Self::export_written`]); one still moving data
        // when its completion instant passes stays `RUNNING` until it has.
        if !self.export_written {
            return;
        }
        if let Some(due) = self.completion_due_ms {
            if due <= now_ms {
                self.status_code = EXPORT_TASK_STATUS_COMPLETED.to_string();
                // The status this transition puts the task in carries its own message, so it
                // is stamped WITH the status, exactly as the cancel transition stamps the one
                // its status carries.
                self.status_message = Some(EXPORT_TASK_COMPLETED_STATUS_MESSAGE.to_string());
                self.completion_time_ms = Some(due);
                self.completion_due_ms = None;
            }
        }
    }

    /// This task as the model's `ExportTask` structure, which is what `DescribeExportTasks`
    /// reports.
    pub fn reported(&self) -> crate::ops::types::ExportTask {
        crate::ops::types::ExportTask {
            task_id: Some(self.task_id.clone()),
            task_name: self.task_name.clone(),
            log_group_name: Some(self.log_group_name.clone()),
            from: Some(self.from_ms),
            to: Some(self.to_ms),
            destination: Some(self.destination.clone()),
            destination_prefix: Some(self.destination_prefix.clone()),
            status: Some(crate::ops::types::ExportTaskStatus {
                code: Some(self.status_code.clone()),
                message: self.status_message.clone(),
            }),
            execution_info: Some(crate::ops::types::ExportTaskExecutionInfo {
                creation_time: Some(self.creation_time_ms),
                completion_time: self.completion_time_ms,
            }),
        }
    }
}

/// The id `CreateImportTask` gives the task it creates — "A unique identifier for the import
/// task" — minted from the store's monotonic import-task counter so it is unique for the life
/// of the store and DERIVED FROM STORED STATE.
///
/// CloudWatch Logs renders one as a canonical hyphenated UUID, which the modelled `ImportId`
/// (`^[\-a-zA-Z0-9]+$`, 1..256) admits, so it is minted through the shared
/// [`mint_uuid_from_seq`] every UUID-shaped id of this service comes from.
pub fn mint_import_task_id(seq: u64) -> String {
    mint_uuid_from_seq(seq)
}

/// The id `CreateExportTask` gives the task it creates — "The ID of the export task" —
/// minted from the store's monotonic export-task counter so it is unique for the life of the
/// store and DERIVED FROM STORED STATE.
///
/// CloudWatch Logs renders one as a canonical hyphenated UUID, so it is minted through the
/// shared [`mint_uuid_from_seq`] every UUID-shaped id of this service comes from.
pub fn mint_export_task_id(seq: u64) -> String {
    mint_uuid_from_seq(seq)
}

/// The service-managed catalogue of vended-log delivery configuration templates, which
/// `DescribeConfigurationTemplates` reports: the resources that can send vended logs, the log
/// types each of them sends, the record schema of each log type, and how a template's
/// packaging is derived from the destination type.
pub mod vended_log_templates;
/// The service's documented constants: defaults, quotas, value catalogues and the ARN and
/// name fragments the records are built from.
mod catalogue;
/// A metric filter's stored configuration, and the rules a metric transformation is held
/// to before it is stored.
mod metric_filter_config;

// Each of the two modules above holds one concern of this state; every path a caller
// already uses (`crate::state::<NAME>`) resolves through these re-exports.
pub use catalogue::*;
pub use metric_filter_config::*;

/// The logical destinations this account holds — "a cross-account destination that receives
/// subscription log events" — and the access policies that authorize writing to one.
mod destinations;
pub use destinations::*;

pub use vended_log_templates::{
    configuration_template_key, configuration_template_output_formats, configuration_templates,
    ConfigurationTemplateSource, CONFIGURATION_TEMPLATE_DEFAULT_FIELD_DELIMITER,
    CONFIGURATION_TEMPLATE_DEFAULT_HIVE_COMPATIBLE_PATH, CONFIGURATION_TEMPLATE_SOURCES,
};

/// The log types a delivery source over `resource_arn` may send, or `None` when this service
/// publishes none for the resource's service.
///
/// A `logType` "defines the type of log that the source is sending", and which values are legal
/// is a property of the SENDING SERVICE rather than of the member: the `logType` member's own
/// catalogue lists them per service ("For CloudFront, the valid value is ACCESS_LOGS"), which is
/// the same catalogue [`CONFIGURATION_TEMPLATE_SOURCES`] is built from, keyed by the service
/// namespace of the resource's ARN ([`delivery_source_service`]). A resource whose service that
/// catalogue does not name publishes no list here, so no log type can be judged against it.
///
/// One service sends the log types of EVERY row the catalogue holds for it — a service has one
/// row per kind of resource, and a log type one of its resources sends is one that service
/// sends — listed in the order the catalogue states them and without repeating a type two of
/// its rows share.
pub fn delivery_source_log_types(resource_arn: &str) -> Option<Vec<&'static str>> {
    let service = delivery_source_service(resource_arn)?;
    let mut published: Vec<&'static str> = Vec::new();
    for source in CONFIGURATION_TEMPLATE_SOURCES
        .iter()
        .filter(|source| source.service == service)
    {
        for entry in source.log_types {
            if !published.contains(&entry.log_type) {
                published.push(entry.log_type);
            }
        }
    }
    (!published.is_empty()).then_some(published)
}

/// Every log type the vended-log catalogue names, ACROSS every sending service: the log
/// sources this delivery plane knows of at all, whichever service sends each of them.
///
/// Source: the `logType` member of `PutDeliverySourceRequest`
/// (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`), read as
/// one list rather than per service — "For Amazon EventBridge Event Bus (Custom), the valid
/// values are INFO_LOGS and ERROR_LOGS", "For Amazon Web Services Shield Advanced, the valid
/// value is FLOW_LOGS", and so on for every row — in the order that catalogue states them and
/// without repeating a type two of its rows share.
pub const VENDED_LOG_TYPES: &[&str] = &[
    "ACCESS_LOGS",
    "WAF_LOGS",
    "ALB_ACCESS_LOGS",
    "ALB_CONNECTION_LOGS",
    "ALB_HEALTH_CHECK_LOGS",
    "APPLICATION_LOGS",
    "TRACES",
    "USAGE_LOGS",
    "EVENT_LOGS",
    "INSIGHTS_QUERY_LOGS",
    "AUTO_MODE_BLOCK_STORAGE_LOGS",
    "AUTO_MODE_COMPUTE_LOGS",
    "AUTO_MODE_IPAM_LOGS",
    "AUTO_MODE_LOAD_BALANCING_LOGS",
    "EKS_CAPABILITY_ACK_LOGS",
    "EKS_CAPABILITY_ARGOCD_APPLICATION_LOGS",
    "EKS_CAPABILITY_ARGOCD_APPLICATIONSET_LOGS",
    "EKS_CAPABILITY_ARGOCD_COMMITSERVER_LOGS",
    "EKS_CAPABILITY_ARGOCD_REPOSERVER_LOGS",
    "EKS_CAPABILITY_ARGOCD_SERVER_LOGS",
    "EKS_CAPABILITY_KRO_LOGS",
    "EGRESS_ACCESS_LOGS",
    "INGRESS_ACCESS_LOGS",
    "AD_DECISION_SERVER_LOGS",
    "MANIFEST_SERVICE_LOGS",
    "TRANSCODE_LOGS",
    "ACTION_LOGS",
    "WORKFLOW_LOGS",
    "INFO_LOGS",
    "ERROR_LOGS",
    "ALERT_LOGS",
    "ALLOW_LOGS",
    "DENY_LOGS",
    "NLB_ACCESS_LOGS",
    "PCS_SCHEDULER_LOGS",
    "PCS_JOBCOMP_LOGS",
    "PCS_SCHEDULER_AUDIT_LOGS",
    "PCS_SCALING_LOGS",
    "SYNC_JOB_LOGS",
    "AGENT_HOURS_LOGS",
    "AGENT_METADATA_LOGS",
    "CHAT_LOGS",
    "DLP_LOGS",
    "FEEDBACK_LOGS",
    "INDEX_USAGE_LOGS",
    "KB_FILE_SYNC_LOGS",
    "GLOBAL_RESOLVER_LOGS",
    "S3_SERVER_ACCESS_LOGS",
    "SECURITY_FINDING_LOGS",
    "TRAFFIC_POLICY_DEBUG_LOGS",
    "FLOW_LOGS",
    "ACCESS_CONTROL_LOGS",
    "AUTHENTICATION_LOGS",
    "WORKMAIL_AVAILABILITY_PROVIDER_LOGS",
    "WORKMAIL_MAILBOX_ACCESS_LOGS",
    "WORKMAIL_PERSONAL_ACCESS_TOKEN_LOGS",
];

/// Whether `log_type` names a log source this delivery plane knows at all — one some service
/// sends, whether or not it is the service of the resource a put names.
///
/// Both catalogues this service publishes answer it: the per-service lists of
/// [`VENDED_LOG_TYPES`], and the configuration-template rows
/// ([`CONFIGURATION_TEMPLATE_SOURCES`]), which carry a type for a sender the `logType` member's
/// prose does not name.
pub fn is_vended_log_type(log_type: &str) -> bool {
    VENDED_LOG_TYPES.contains(&log_type)
        || CONFIGURATION_TEMPLATE_SOURCES.iter().any(|source| {
            source
                .log_types
                .iter()
                .any(|entry| entry.log_type == log_type)
        })
}

/// The refusal a put naming a log source whose logs the named resource's service does not send
/// is answered with: the type IS a log source ([`is_vended_log_type`]) — some other service
/// sends it — so what is wrong is the pairing, and the sentence states exactly that.
///
/// Entirely FIXED text for that error class: it names neither the type, nor the resource, nor
/// the service, nor the types that service does send, so every mismatched pairing is refused
/// identically.
pub const LOG_TYPE_SERVICE_NOT_ALLOWED_MESSAGE: &str =
    "This service is not allowed for this LogSource.";

/// The refusal for a `logType` the resource named by `resource_arn` does not send
/// ([`delivery_source_log_types`]), or `None` when it is one of them — or when this service
/// publishes no list for that resource's service, which is no basis to refuse one.
///
/// Which of the two sentences the class is refused with turns on whether the type names a log
/// source at all ([`is_vended_log_type`]):
///
/// * a type SOME service sends is a log source this pairing is not allowed for, so the refusal
///   is the fixed [`LOG_TYPE_SERVICE_NOT_ALLOWED_MESSAGE`] — the type is valid, the sending
///   service is the part that is not;
/// * a type no service sends is no log source at all, so the refusal states that the provided
///   log type is not valid and then the options that ARE, which is its one variable part: the
///   log types the named resource's service sends, in the order the catalogue lists them.
pub fn unsupported_log_type_message(resource_arn: &str, log_type: &str) -> Option<String> {
    let supported = delivery_source_log_types(resource_arn)?;
    if supported.contains(&log_type) {
        return None;
    }
    Some(if is_vended_log_type(log_type) {
        LOG_TYPE_SERVICE_NOT_ALLOWED_MESSAGE.to_string()
    } else {
        format!(
            "Provided LogType is not valid. Supported options are [{}].",
            supported.join(", ")
        )
    })
}

/// The ARN service namespaces of the GLOBAL log senders: a resource of one of these lives in
/// its partition rather than in a Region, which is why an ARN naming one carries an EMPTY
/// Region segment, and each is reached through a single endpoint per partition rather than one
/// per Region.
///
/// Source: these are the senders the `logType` member's own catalogue names that are global —
/// "For CloudFront, the valid value is ACCESS_LOGS", "For Route 53 Global Resolver, the valid
/// value is GLOBAL_RESOLVER_LOGS", "For Amazon Web Services Shield Advanced, the valid value
/// is FLOW_LOGS"
/// (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`) — and the
/// documented ARN form of each carries no Region
/// (`arn:aws:cloudfront::<account>:distribution/<id>`, `arn:aws:route53:::hostedzone/<id>`,
/// `arn:aws:shield::<account>:protection/<id>`). A global vendor the catalogue does not name
/// as a sender is left out rather than extrapolated: some global services publish their one
/// endpoint in a Region other than the partition's usual global endpoint, so each would need
/// its own.
///
/// A Region-less ARN is NOT by itself the mark of a global resource — an Amazon S3 bucket ARN
/// (`arn:aws:s3:::<bucket>`) carries no Region either, and a bucket is regional — so this is a
/// list of namespaces rather than a test on the ARN's Region segment.
pub const GLOBAL_SERVICE_VENDORS: [&str; 3] = ["cloudfront", "route53", "shield"];

/// The one Region each partition's global senders above are reached through, keyed by the
/// partition segment of an ARN.
///
/// Source: the global endpoint each partition publishes for these services — the commercial
/// partition's in US East (N. Virginia), the China partitions' in Beijing, the AWS GovCloud
/// (US) partition's in US-Gov-West. A partition this does not name pins no Region, so nothing
/// is refused for it.
pub const PARTITION_GLOBAL_REGIONS: [(&str, &str); 3] = [
    ("aws", "us-east-1"),
    ("aws-cn", "cn-north-1"),
    ("aws-us-gov", "us-gov-west-1"),
];

/// The partition segment of an ARN (`arn:<partition>:...`), or `None` for a value that is not
/// an ARN at all. One split of an ARN's fields serves the whole crate ([`arn_parts`]).
fn arn_partition(arn: &str) -> Option<&str> {
    arn_parts(arn).map(|(partition, _service, _account, _resource)| partition)
}

/// The Region a delivery source for `resource_arn` can be created in, or `None` when the
/// resource it names pins none.
///
/// A resource of a global service ([`GLOBAL_SERVICE_VENDORS`]) is served by one endpoint for
/// the whole partition ([`PARTITION_GLOBAL_REGIONS`]), so that endpoint's Region is the only
/// one that can hold a delivery source for it. Everything else — a regional resource, or one
/// of a partition with no global endpoint named here — is left unpinned.
pub fn delivery_source_required_region(resource_arn: &str) -> Option<&'static str> {
    let (vendor, _resource) = arn_service_and_resource(resource_arn)?;
    if !GLOBAL_SERVICE_VENDORS.contains(&vendor) {
        return None;
    }
    let partition = arn_partition(resource_arn)?;
    PARTITION_GLOBAL_REGIONS
        .iter()
        .find(|(name, _region)| *name == partition)
        .map(|(_name, region)| *region)
}

/// The refusal for a delivery source whose resource is reached through another Region than the
/// one this request was made in ([`delivery_source_required_region`]), or `None` when the
/// request's Region can hold it.
///
/// What the caller is told is that the ACTION is not served for this kind of log in the Region
/// the request reached: the message names the action and that Region, and the literal word for
/// the log-type parameter rather than its value. Nothing else is interpolated — not the
/// resource, not the Region it is reached through, and not the constraint-violation prefix this
/// service puts on a parameter that fails a modelled constraint, which this refusal is not.
pub fn unsupported_log_type_region_message(
    operation: &str,
    resource_arn: &str,
    request_region: &str,
) -> Option<String> {
    let required = delivery_source_required_region(resource_arn)?;
    (required != request_region).then(|| {
        format!("{operation} action is not supported for this LogType in region {request_region}")
    })
}

/// One created log group and everything stored under it.
#[derive(Debug, Clone)]
pub struct LogGroupRecord {
    pub name: String,
    /// The ARN version that carries a trailing `:*` (the `LogGroup$arn` member).
    pub arn: String,
    /// The ARN version without the trailing `:*` (the `LogGroup$logGroupArn` member).
    pub log_group_arn: String,
    pub creation_time_ms: i64,
    pub retention_in_days: Option<i32>,
    pub kms_key_id: Option<String>,
    pub log_group_class: String,
    pub deletion_protection_enabled: bool,
    /// Whether bearer token authentication is enabled for this group
    /// (`LogGroup$bearerTokenAuthenticationEnabled`: "Indicates whether bearer token
    /// authentication is enabled for this log group. When enabled, bearer token
    /// authentication is allowed on operations until it is explicitly disabled.").
    /// `PutBearerTokenAuthentication` is what moves it; a new group is not enabled.
    pub bearer_token_authentication_enabled: bool,
    pub stored_bytes: i64,
    pub tags: BTreeMap<String, String>,
    /// The group's log streams, keyed by `logStreamName`.
    pub streams: BTreeMap<String, LogStreamRecord>,
    /// The group's subscription filters, keyed by `filterName`. A `BTreeMap` so
    /// `DescribeSubscriptionFilters` enumerates them ASCII-sorted by filter name
    /// without re-sorting, and so a put under a name already stored updates that
    /// filter in place.
    pub subscription_filters: BTreeMap<String, SubscriptionFilterRecord>,
    /// The group's metric filters, keyed by `filterName`. A `BTreeMap` so
    /// `DescribeMetricFilters` enumerates them ASCII-sorted by filter name without
    /// re-sorting, and so a put under a name already stored updates that filter in place.
    /// They live on the group `PutMetricFilter` "associates" them with, so deleting the
    /// group removes its metric filters too.
    pub metric_filters: BTreeMap<String, MetricFilterRecord>,
    /// The resource-scoped policy attached to this group, if one has been put: AWS
    /// allows "one per LogGroup resourceARN", so at most one.
    pub resource_policy: Option<ResourcePolicyRecord>,
    /// The log-group level transformer `PutTransformer` created for this group, if one
    /// has been put. A log group has at most one — `PutTransformer` "creates or updates
    /// a log transformer for a single log group" and names it by nothing but the group —
    /// and it lives on the group, so deleting the group takes its transformer with it.
    pub transformer: Option<TransformerRecord>,
    /// The data protection policy attached to this group, if one has been put:
    /// `PutDataProtectionPolicy` "applies to only the specified log group" and a put
    /// replaces the policy the group holds, so at most one. `None` again after
    /// `DeleteDataProtectionPolicy`, and gone with the group when it is deleted.
    pub data_protection_policy: Option<DataProtectionPolicyRecord>,
    /// Whether this group has EVER held a data protection policy, which is what lets
    /// `LogGroup$dataProtectionStatus` report "whether this log group has a protection
    /// policy, or whether it had one in the past". See
    /// [`LogGroupRecord::data_protection_status`].
    pub had_data_protection_policy: bool,
    /// The data sources that ingest into this group — what `ListLogGroups`'s
    /// `dataSources` filter selects on and what `ListAggregateLogGroupSummaries`
    /// counts log groups by. See [`DataSourceRecord`] for why a group created through
    /// this emulator's API carries none.
    pub data_sources: Vec<DataSourceRecord>,
    /// The syslog configurations put on this group, keyed by the `vpcEndpointId` the
    /// configuration ingests through — `PutSyslogConfiguration` "enables ingestion of
    /// syslog data through the specified VPC endpoint into the log group", so one group
    /// holds at most one configuration per endpoint and a put under an endpoint already
    /// stored updates that configuration in place. A `BTreeMap` so
    /// `ListSyslogConfigurations` enumerates them ASCII-sorted by endpoint id without
    /// re-sorting. Every configuration names an endpoint ([`VPC_ENDPOINT_ID_REQUIRED`]).
    pub syslog_configurations: BTreeMap<String, SyslogConfigurationRecord>,
    /// The log-group level field index policy stored on this group, if one has been put:
    /// such a policy is named by nothing but the group, so a group holds at most one and
    /// deleting the group takes it away. `DescribeIndexPolicies` reports this policy for
    /// the group ahead of any account-wide one ([`IndexPolicyRecord`]).
    pub index_policy: Option<IndexPolicyRecord>,
}

impl LogGroupRecord {
    /// A brand-new log group: the class default, no retention, no stored bytes.
    pub fn new(region: &str, name: &str, creation_time_ms: i64) -> Self {
        Self {
            name: name.to_string(),
            arn: log_group_arn(region, name, true),
            log_group_arn: log_group_arn(region, name, false),
            creation_time_ms,
            retention_in_days: None,
            kms_key_id: None,
            log_group_class: DEFAULT_LOG_GROUP_CLASS.to_string(),
            deletion_protection_enabled: false,
            bearer_token_authentication_enabled: false,
            stored_bytes: 0,
            tags: BTreeMap::new(),
            streams: BTreeMap::new(),
            subscription_filters: BTreeMap::new(),
            metric_filters: BTreeMap::new(),
            resource_policy: None,
            transformer: None,
            data_protection_policy: None,
            had_data_protection_policy: false,
            data_sources: Vec::new(),
            syslog_configurations: BTreeMap::new(),
            index_policy: None,
        }
    }

    /// This group's `metricFilterCount` — "The number of metric filters" — which is the
    /// number of metric filters stored on it: `PutMetricFilter` associates one with a log
    /// group and `DeleteMetricFilter` takes it away, so the count is DERIVED from
    /// [`LogGroupRecord::metric_filters`] rather than kept beside it, and no operation can
    /// move the two apart.
    pub fn metric_filter_count(&self) -> i32 {
        i32::try_from(self.metric_filters.len()).unwrap_or(i32::MAX)
    }

    /// The regular expression patterns this group's metric filters carry between them,
    /// EXCLUDING the filter named by `replacing` (a put under a name the group already
    /// holds updates that filter, so the pattern it carried today is replaced rather than
    /// added to). This is the quantity `PutMetricFilter`'s "quota of five regular
    /// expression patterns per log group" bounds.
    pub fn metric_filter_regex_patterns(&self, replacing: &str) -> usize {
        self.metric_filters
            .values()
            .filter(|filter| filter.filter_name != replacing)
            .map(|filter| filter_pattern_regex_count(&filter.filter_pattern))
            .sum()
    }

    /// This group's `dataProtectionStatus`: `ACTIVATED` while it holds a data protection
    /// policy, `DELETED` once one it held has been deleted, and absent for a group that
    /// has never held one — the member "displays whether this log group has a protection
    /// policy, or whether it had one in the past", so a group with neither has nothing to
    /// report. The other two values of the `DataProtectionStatus` enum belong to the
    /// account-level policies of `PutAccountPolicy`
    /// ([`ComAmazonawsCloudwatchlogsLogs20140328State::account_policies`]), which no
    /// operation of this service moves a per-group status for yet.
    pub fn data_protection_status(&self) -> Option<&'static str> {
        if self.data_protection_policy.is_some() {
            Some(DATA_PROTECTION_STATUS_ACTIVATED)
        } else if self.had_data_protection_policy {
            Some(DATA_PROTECTION_STATUS_DELETED)
        } else {
            None
        }
    }
}

/// The one value the model's `SyslogSourceType` enum admits
/// ([`crate::ops::types::SYSLOG_SOURCE_TYPE_VALUES`]), and the source a syslog
/// configuration ingests through: the VPC endpoint `PutSyslogConfiguration` names. Kept as
/// a named constant so the stamped `SyslogConfiguration$sourceType` is the model's value
/// rather than a literal typed into a handler; `tests/all/syslog_configurations.rs` pins
/// what a listed configuration reports to that catalogue.
pub const SYSLOG_SOURCE_TYPE: &str = "VPCE";

/// The key a syslog configuration is stored under: its `vpcEndpointId`. The member is
/// OPTIONAL in the model on both `PutSyslogConfiguration` and
/// `DeleteSyslogConfiguration`, but a syslog configuration IS the ingestion a log group
/// accepts through one VPC endpoint — the only `SyslogSourceType` the model admits is
/// `VPCE`, and the endpoint is the only thing that distinguishes one configuration on a log
/// group from another — so the service refuses a request that names none. This is that
/// refusal's sentence: the service's own fixed wording for the constraint, naming the member
/// and what it requires.
pub const VPC_ENDPOINT_ID_REQUIRED: &str = "vpcEndpointId is required";

/// One syslog configuration stored on a log group: the model's `SyslogConfiguration`
/// minus the `logGroupArn` the owning group already carries.
#[derive(Debug, Clone)]
pub struct SyslogConfigurationRecord {
    /// `SyslogConfiguration$vpcEndpointId` — "The ID of the VPC endpoint used for syslog
    /// ingestion", which is also the key the owning group stores this configuration under
    /// (see [`VPC_ENDPOINT_ID_REQUIRED`]).
    pub vpc_endpoint_id: String,
    /// `SyslogConfiguration$sourceType` — "The source type for the syslog
    /// configuration": [`SYSLOG_SOURCE_TYPE`].
    pub source_type: String,
    /// `SyslogConfiguration$createdAt` — "The time when the syslog configuration was
    /// created, expressed as the number of milliseconds after Jan 1, 1970 00:00:00 UTC".
    /// A put over a configuration already stored UPDATES it, so this keeps the instant the
    /// first put created it.
    pub created_at_ms: i64,
}

/// One data source that ingests into a log group: the model's `DataSource` — "a data
/// source that categorizes logs by originating service and log type" — plus the format
/// the aggregate summaries can additionally group by (`dataSource.Format`).
///
/// A log group created through this emulator's API carries none, and that is a property
/// of the API surface rather than a shortcut: the model's `LogGroup` has no data-source
/// member, no operation associates a data source WITH a log group (the one operation that
/// names a `DataSource`, `AssociateSourceToS3TableIntegration`, attaches it to an S3
/// Tables integration — [`S3TableIntegrationSourceRecord`] — rather than to a log group),
/// and the data sources AWS reports are the ones
/// its own vended-log ingestion created. So the aggregate grouped by data-source
/// characteristics counts nothing here — which is a 200 with an empty list, not a
/// refusal — and both readers go through this field, so they report whatever a later
/// operation stores in it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DataSourceRecord {
    /// `DataSource$name` — "The name of the data source."
    pub name: String,
    /// `DataSource$type` — "The type of the data source."
    pub source_type: Option<String>,
    /// The data source's format, which `groupBy=DATA_SOURCE_NAME_TYPE_AND_FORMAT`
    /// groups by (`GroupingIdentifier`: "Examples are, dataSource.Name,
    /// dataSource.Type, and dataSource.Format").
    pub format: Option<String>,
}

/// Whether one data source satisfies a `dataSources` filter list.
///
/// From `ListLogGroups$dataSources`: "You can filter by data source name, type, or both.
/// Multiple filters within the same dimension are combined with OR logic, while filters
/// across different dimensions are combined with AND logic" — so every filter's `name`
/// forms one dimension and every filter's `type` the other, and each is matched as the
/// "name pattern" / "type pattern" the members document: a case-sensitive substring.
pub fn data_source_matches_filters(
    source: &DataSourceRecord,
    filters: &[crate::ops::types::DataSourceFilter],
) -> bool {
    // `DataSourceFilter$name` is `@required`, so the name dimension is never empty.
    let name_matches = filters
        .iter()
        .any(|filter| source.name.contains(&filter.name));
    let wanted_types: Vec<&str> = filters
        .iter()
        .filter_map(|filter| filter.r#type.as_deref())
        .collect();
    let type_matches = wanted_types.is_empty()
        || source.source_type.as_deref().is_some_and(|source_type| {
            wanted_types
                .iter()
                .any(|wanted| source_type.contains(wanted))
        });
    name_matches && type_matches
}

/// Whether one log group satisfies a `dataSources` filter list: one of its data sources
/// does ([`data_source_matches_filters`]).
pub fn log_group_matches_data_source_filters(
    group: &LogGroupRecord,
    filters: &[crate::ops::types::DataSourceFilter],
) -> bool {
    group
        .data_sources
        .iter()
        .any(|source| data_source_matches_filters(source, filters))
}

/// Whether one character may appear in a `logGroupNamePattern` alternative, from the
/// `LogGroupNameRegexPattern` `@pattern`'s character class `[\.\-_\/#A-Za-z0-9]` — which
/// is exactly the class a `LogGroupName` is built from, so an alternative can only ever
/// be literal text.
fn is_log_group_name_pattern_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '/' | '#')
}

/// The `LogGroupNameRegexPattern` expression as the SERVICE states it in a refusal, which
/// is not the trait string the model carries: it is stated UNANCHORED, the form the
/// pattern-rendering rule of `requirement` in `crate::service` states for every shape AWS
/// publishes another form for, and it carries one alternative the model omits — a SINGLE
/// pattern of 3..512 characters, which is the form the service accepts for a pattern naming
/// a whole log group name and which the model's `{3,24}` segments alone would exclude.
///
/// It still includes the per-alternative 24-character maximum of the multi-pattern
/// alternative, which the emulator does not enforce for the same reason the service does
/// not (see [`check_log_group_name_pattern`]).
pub const LOG_GROUP_NAME_REGEX_PATTERN: &str = concat!(
    r"(\^?[\.\-_\/#A-Za-z0-9]{3,512})",
    r"|(\^?[\.\-_\/#A-Za-z0-9]{3,24})(\|\^?[\.\-_\/#A-Za-z0-9]{3,24}){0,4}"
);

/// One refused member, worded on the template CloudWatch Logs words a constraint
/// violation with: "N validation error detected: Value at '<member>' failed to satisfy
/// constraint: <constraint>". The template is built from the request — the member that
/// broke a constraint, and that member's modelled constraint — and carries NOTHING of the
/// submitted value.
///
/// Delegates to `crate::service`, which owns that template for every refusal of this
/// service, so a violation a handler finds and one the boundary walk finds read alike.
pub fn validation_message(member: &str, constraint: &str) -> String {
    crate::service::one_violation_message(member, constraint)
}

/// Judge a `logGroupNamePattern` — the member `ListLogGroups` and
/// `ListAggregateLogGroupSummaries` share — answering the message to refuse it with.
///
/// The member targets `LogGroupNameRegexPattern`, whose `@pattern` decomposes the value
/// into up to five `|`-separated alternatives of 3..24 characters. The 24-character
/// MAXIMUM is the one part of that the service does not apply: it answers 200 for a
/// single-alternative pattern far longer than 24 characters, which is what a pattern
/// naming a real log group looks like. Everything else the model and the documentation
/// state is enforced here, because the member carries `skip_validation` and the boundary
/// walk no longer sees it:
///
/// * the `@length(min: 3, max: 129)` on the whole value;
/// * "as many as five different regular expression patterns in this field";
/// * the minimum of "each of which must be between 3 and 24 characters";
/// * the character class, plus the optional leading `^` anchor on each alternative.
pub fn check_log_group_name_pattern(pattern: &str) -> Result<(), String> {
    const MEMBER: &str = "logGroupNamePattern";
    let refuse = |constraint: String| Err(validation_message(MEMBER, &constraint));

    let length = pattern.chars().count();
    if length < LOG_GROUP_NAME_PATTERN_MIN_LEN {
        return refuse(format!(
            "Member must have length greater than or equal to {LOG_GROUP_NAME_PATTERN_MIN_LEN}"
        ));
    }
    if length > LOG_GROUP_NAME_PATTERN_MAX_LEN {
        return refuse(format!(
            "Member must have length less than or equal to {LOG_GROUP_NAME_PATTERN_MAX_LEN}"
        ));
    }
    let alternatives: Vec<&str> = pattern.split(LOG_GROUP_NAME_PATTERN_SEPARATOR).collect();
    // Every shape the value can break beyond its length — more alternatives than the
    // grammar admits, an alternative that is too short, and a character outside the
    // class — is one expression it does not satisfy, so each is stated as the `@pattern`
    // constraint it is, naming the expression itself rather than a prose rule of this
    // emulator's own.
    if alternatives.len() > MAX_LOG_GROUP_NAME_PATTERN_ALTERNATIVES {
        return refuse(pattern_constraint());
    }
    for alternative in alternatives {
        let literal = alternative
            .strip_prefix(LOG_GROUP_NAME_PATTERN_ANCHOR)
            .unwrap_or(alternative);
        let shaped = literal.chars().count() >= MIN_LOG_GROUP_NAME_PATTERN_ALTERNATIVE_LEN
            && literal.chars().all(is_log_group_name_pattern_char);
        if !shaped {
            return refuse(pattern_constraint());
        }
    }
    Ok(())
}

/// The `@pattern` clause a `logGroupNamePattern` outside the accepted grammar is refused
/// with: the one constraint sentence, stating the expression the service publishes
/// ([`LOG_GROUP_NAME_REGEX_PATTERN`]).
fn pattern_constraint() -> String {
    format!("Member must satisfy regular expression pattern: {LOG_GROUP_NAME_REGEX_PATTERN}")
}

/// Whether a log group name matches a `logGroupNamePattern`.
///
/// The member documents the whole match rule: "Use ^ to match log group names by prefix.
/// For a substring match, specify the string to match. All matches are case sensitive To
/// match multiple patterns, separate them with a | as in the example ^/aws/lambda|
/// discovery". The pattern's character class admits no regular-expression metacharacter
/// beyond that `^` and `|`, so each alternative is literal text: anchored, a prefix;
/// otherwise, a substring. A name matches when ANY alternative matches it.
///
/// Caller-side the value has already been through [`check_log_group_name_pattern`].
pub fn log_group_name_matches_pattern(name: &str, pattern: &str) -> bool {
    pattern
        .split(LOG_GROUP_NAME_PATTERN_SEPARATOR)
        .any(|alternative| {
            match alternative.strip_prefix(LOG_GROUP_NAME_PATTERN_ANCHOR) {
                Some(prefix) => name.starts_with(prefix),
                None => name.contains(alternative),
            }
        })
}

/// One log-group level transformer — "a series of processors, where each processor
/// applies one type of transformation to the log events ingested into this log group" —
/// as it is stored, on the single log group it transforms for.
#[derive(Debug, Clone)]
pub struct TransformerRecord {
    /// The processors, in the order the put listed them: "The processors work one after
    /// another, in the order that you list them, like a pipeline", so the order is part
    /// of the configuration and a read reports it back unchanged.
    pub processors: Vec<crate::ops::types::Processor>,
    /// When this transformer was first created for the group. An update keeps it: a put
    /// over an existing transformer is the same transformer, reconfigured.
    pub creation_time_ms: i64,
    /// When the transformer was last put, which for a transformer created and never
    /// updated is its creation time.
    pub last_modified_time_ms: i64,
}

/// The transformations one processor names, as the model spells the members that carry
/// them.
///
/// `Processor` is "the information about one processor in a log transformer" and a
/// `transformerConfig` is "an array of processors, where each processor applies one type
/// of transformation", so exactly one of the structure's members is populated. Reporting
/// the populated ones lets a put refuse a processor that names none, and one that names
/// several, with a message that lists what it named.
pub fn processor_transformations(processor: &crate::ops::types::Processor) -> Vec<&'static str> {
    // Destructured EXHAUSTIVELY — no `..` — so a processor kind the model gains is a
    // COMPILE error here rather than a member this function silently cannot see, which
    // would make a put naming only that kind look like a processor naming none.
    let crate::ops::types::Processor {
        add_keys,
        copy_value,
        csv,
        date_time_converter,
        delete_keys,
        grok,
        list_to_map,
        lower_case_string,
        move_keys,
        parse_cloudfront,
        parse_json,
        parse_key_value,
        parse_route53,
        parse_to_ocsf,
        parse_postgres,
        parse_vpc,
        parse_waf,
        rename_keys,
        split_string,
        substitute_string,
        trim_string,
        type_converter,
        upper_case_string,
    } = processor;
    // The member names are the model's own (`Processor`'s members), in model order.
    [
        ("addKeys", add_keys.is_some()),
        ("copyValue", copy_value.is_some()),
        ("csv", csv.is_some()),
        ("dateTimeConverter", date_time_converter.is_some()),
        ("deleteKeys", delete_keys.is_some()),
        ("grok", grok.is_some()),
        ("listToMap", list_to_map.is_some()),
        ("lowerCaseString", lower_case_string.is_some()),
        ("moveKeys", move_keys.is_some()),
        ("parseCloudfront", parse_cloudfront.is_some()),
        ("parseJSON", parse_json.is_some()),
        ("parseKeyValue", parse_key_value.is_some()),
        ("parseRoute53", parse_route53.is_some()),
        ("parseToOCSF", parse_to_ocsf.is_some()),
        ("parsePostgres", parse_postgres.is_some()),
        ("parseVPC", parse_vpc.is_some()),
        ("parseWAF", parse_waf.is_some()),
        ("renameKeys", rename_keys.is_some()),
        ("splitString", split_string.is_some()),
        ("substituteString", substitute_string.is_some()),
        ("trimString", trim_string.is_some()),
        ("typeConverter", type_converter.is_some()),
        ("upperCaseString", upper_case_string.is_some()),
    ]
    .into_iter()
    .filter_map(|(member, populated)| populated.then_some(member))
    .collect()
}

/// The processor kinds that PARSE a log event — the ones that turn the raw message into
/// the structured event the other processors work on — as the model spells the `Processor`
/// members that carry them.
///
/// A transformer "must have at least one parse-type processor" (`ParseJSON`), and a
/// configuration whose FIRST processor is a mutation step structures nothing for the rest of
/// the pipeline to read, which is what [`transformer_leading_parser_message`] refuses. The
/// set is the configurable parsers (`csv`, `grok`, `parseJSON`, `parseKeyValue`) and the
/// parsers for AWS-vended log formats, each of which the model documents as parsing a log
/// event into fields; it is keyed on the member name, so it holds for every operation that
/// takes a `Processors` list.
pub const PARSE_TYPE_PROCESSORS: &[&str] = &[
    "csv",
    "grok",
    "parseCloudfront",
    "parseJSON",
    "parseKeyValue",
    "parsePostgres",
    "parseRoute53",
    "parseToOCSF",
    "parseVPC",
    "parseWAF",
];

/// The processor kinds that may appear ONLY as the first processor of a transformer, as the
/// model states on each of them: "If you use this processor, it must be the first processor
/// in your transformer" (`ParseCloudfront`, `ParseRoute53`, `ParsePostgres`, `ParseVPC`,
/// `ParseWAF`). Each "always processes the entire log event message", so there is nothing
/// for it to do after another processor has already restructured the event.
pub const LEADING_ONLY_PROCESSORS: &[&str] = &[
    "parseCloudfront",
    "parsePostgres",
    "parseRoute53",
    "parseVPC",
    "parseWAF",
];

/// Whether the processor member `member` names a parse-type processor
/// ([`PARSE_TYPE_PROCESSORS`]).
pub fn is_parse_type_processor(member: &str) -> bool {
    PARSE_TYPE_PROCESSORS.contains(&member)
}

/// The refusal for a transformer whose first processor is not a parse-type one: it states
/// the rule and which kinds satisfy it, and names the kind the request put first.
pub fn transformer_leading_parser_message(named: &str) -> String {
    format!(
        "The first processor in a transformer must be a parse-type processor, one of {}; \
         this transformer begins with {named}.",
        PARSE_TYPE_PROCESSORS.join(", ")
    )
}

/// The refusal for a processor the model admits only as the first one of a transformer
/// ([`LEADING_ONLY_PROCESSORS`]), used at any later position.
///
/// The sentence is the service's own for that error class — a log processor which has to lead
/// must be in the first position of the processor list — and its one variable part is the
/// processor kind the request put later, as the request named it. Neither the position it was
/// found at nor anything else of the request is interpolated.
pub fn transformer_leading_only_message(named: &str) -> String {
    format!("Log processor {named} must be in the first position of the processor list")
}

/// One saved CloudWatch Logs Insights query definition — "details about a saved
/// CloudWatch Logs Insights query" — as it is stored.
///
/// Every member a put names is replaced wholesale on an update, because
/// `PutQueryDefinition` states that "the values of name, queryString, and logGroupNames
/// are changed to the values that you specify in your update operation. No current values
/// are retained from the current query definition."
#[derive(Debug, Clone)]
pub struct QueryDefinitionRecord {
    /// The id CloudWatch generated for this definition, kept on the record as well as in
    /// the map key so a report built by enumerating the map carries it.
    pub query_definition_id: String,
    pub name: String,
    pub query_string: String,
    /// The language this definition's query string is written in. Never absent: a put
    /// that names no `queryLanguage` saves the definition with
    /// [`DEFAULT_QUERY_LANGUAGE`], so `DescribeQueryDefinitions$queryLanguage` —
    /// "limits the returned queries to only the queries that use the specified query
    /// language" — can place every stored definition.
    pub query_language: String,
    /// The log groups the definition "is limited to", when the put named any; `None`
    /// when it named none, which `QueryDefinition$logGroupNames` reports by omitting the
    /// member ("If this query definition contains a list of log groups that it is
    /// limited to, that list appears here").
    pub log_group_names: Option<Vec<String>>,
    /// The placeholder variables the definition's query string references, when the put
    /// named any; `None` otherwise, on the same rule as `logGroupNames`.
    pub parameters: Option<Vec<crate::ops::types::QueryParameter>>,
    pub last_modified_ms: i64,
    /// The `clientToken` of the CREATE that minted this definition, so a create retried
    /// "twice because of a network error" with the same token answers with this definition
    /// instead of saving a second one. An update never re-points it: the token identifies
    /// the create it de-duplicates, not the definition's current values.
    pub client_token: Option<String>,
}

/// One subscription filter — "a real-time stream of log events ingested through
/// PutLogEvents" delivered to one destination — as it is stored. It lives on the log
/// group it is "associated with", so deleting that group removes its filters with it.
#[derive(Debug, Clone)]
pub struct SubscriptionFilterRecord {
    pub filter_name: String,
    pub filter_pattern: String,
    pub destination_arn: String,
    pub role_arn: Option<String>,
    /// How log data is spread over the destination. Never absent: a put that names no
    /// `distribution` gets the documented default ([`DEFAULT_DISTRIBUTION`]), so a
    /// client reads back the grouping the filter is actually configured with.
    pub distribution: String,
    pub apply_on_transformed_logs: bool,
    pub field_selection_criteria: Option<String>,
    pub emit_system_fields: Option<Vec<String>>,
    pub creation_time_ms: i64,
    /// Whether this filter's destination REFUSED its last delivery
    /// ([`subscription_delivery::deliver`]). A destination that answered a delivery with a
    /// failure is not attempted again: the filter delivers nothing more until a
    /// `PutSubscriptionFilter` re-validates it, which is what makes a destination that comes
    /// back serve the batches written AFTER that put rather than replaying the ones written
    /// while it was gone. Never reported: no operation models a filter's delivery health.
    pub destination_unhealthy: bool,
}

/// One created log stream and its events.
#[derive(Debug, Clone)]
pub struct LogStreamRecord {
    pub name: String,
    pub arn: String,
    pub creation_time_ms: i64,
    pub first_event_timestamp: Option<i64>,
    pub last_event_timestamp: Option<i64>,
    pub last_ingestion_time: Option<i64>,
    /// `None` until the first `PutLogEvents`: AWS reports no `uploadSequenceToken`
    /// for a stream nothing has been written to.
    pub upload_sequence_token: Option<String>,
    /// Events kept sorted by `(timestamp, ingestion_seq)`.
    pub events: Vec<StoredEvent>,
}

impl LogStreamRecord {
    /// A brand-new, empty log stream.
    pub fn new(region: &str, group_name: &str, name: &str, creation_time_ms: i64) -> Self {
        Self {
            name: name.to_string(),
            arn: log_stream_arn(region, group_name, name),
            creation_time_ms,
            first_event_timestamp: None,
            last_event_timestamp: None,
            last_ingestion_time: None,
            upload_sequence_token: None,
            events: Vec::new(),
        }
    }

    /// Re-derive the event-time summary members from the stored events.
    pub fn refresh_event_times(&mut self) {
        self.first_event_timestamp = self.events.first().map(|e| e.timestamp);
        self.last_event_timestamp = self.events.last().map(|e| e.timestamp);
    }

    /// The bytes the stream's events occupy, counted the way a log group's
    /// `storedBytes` counts them.
    pub fn stored_bytes(&self) -> i64 {
        self.events
            .iter()
            .map(StoredEvent::stored_bytes)
            .fold(0i64, i64::saturating_add)
    }

    /// Hold the stream inside the emulator's storage ceiling ([`MAX_STREAM_EVENTS`] /
    /// [`MAX_STREAM_BYTES`]) by discarding the oldest events, and answer how many bytes
    /// went, so the owning log group's `storedBytes` can be reduced by the same amount.
    ///
    /// Events are stored oldest-first, so the discarded run is a prefix. A single event
    /// can never exceed the byte ceiling (a log event is capped at 1 MB on the way in),
    /// so the newest event always survives.
    pub fn enforce_storage_ceiling(&mut self) -> i64 {
        let mut kept_bytes = self.stored_bytes();
        let mut discard = 0usize;
        while discard < self.events.len()
            && (self.events.len() - discard > MAX_STREAM_EVENTS || kept_bytes > MAX_STREAM_BYTES)
        {
            kept_bytes = kept_bytes.saturating_sub(self.events[discard].stored_bytes());
            discard += 1;
        }
        let discarded_bytes = self.events[..discard]
            .iter()
            .map(StoredEvent::stored_bytes)
            .fold(0i64, i64::saturating_add);
        self.events.drain(..discard);
        discarded_bytes
    }
}

/// One ingested log event.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub timestamp: i64,
    /// The message as it was INGESTED, byte for byte: what a read with `unmask` reveals.
    pub message: String,
    /// The message as a reader sees it by default, for an event the log group's data
    /// protection policy masked something in when it was ingested
    /// ([`data_protection::masked`]); `None` for an event nothing was masked in, which is
    /// every event of a group with no such policy and every event ingested before one was
    /// put.
    pub masked_message: Option<String>,
    pub ingestion_time: i64,
    pub ingestion_seq: u64,
}

impl StoredEvent {
    /// The total order events are stored and paginated in.
    pub fn key(&self) -> EventKey {
        EventKey {
            timestamp: self.timestamp,
            seq: self.ingestion_seq,
        }
    }

    /// The message a read reports: the ingested bytes where the reader asked to `unmask`
    /// ("Specify true to display the log event fields with all sensitive data unmasked and
    /// visible"), and otherwise the masked version where the event has one — "by default,
    /// when a user views a log event that includes masked data, the sensitive data is
    /// replaced by asterisks".
    pub fn message_as_read(&self, unmask: bool) -> &str {
        match &self.masked_message {
            Some(masked) if !unmask => masked,
            _ => &self.message,
        }
    }

    /// What this event adds to its log group's `storedBytes`: the UTF-8 message plus
    /// the documented 26-byte per-event overhead.
    pub fn stored_bytes(&self) -> i64 {
        i64::try_from(self.message.len()).unwrap_or(i64::MAX) + EVENT_OVERHEAD_BYTES
    }
}

/// A position in a stream's event order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventKey {
    pub timestamp: i64,
    pub seq: u64,
}

impl EventKey {
    /// The position before every event, so a forward scan from it starts at the head.
    pub const BEFORE_ALL: Self = Self {
        timestamp: i64::MIN,
        seq: 0,
    };
    /// The position after every event, so a backward scan from it starts at the tail.
    pub const AFTER_ALL: Self = Self {
        timestamp: i64::MAX,
        seq: u64::MAX,
    };
}

/// The region to stamp into the ARNs this request's resources get: the SigV4
/// credential scope's region, or [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Wall-clock milliseconds since the Unix epoch — what every CloudWatch Logs
/// timestamp member is expressed in.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// A log group's ARN, with the trailing `:*` when `wildcard_suffix` is set (the two
/// versions the model's `arn` and `logGroupArn` members document).
pub fn log_group_arn(region: &str, name: &str, wildcard_suffix: bool) -> String {
    let resource = if wildcard_suffix {
        format!("log-group:{name}:*")
    } else {
        format!("log-group:{name}")
    };
    ::engine::emulator_arn("logs", region, &resource)
}

/// A log stream's ARN.
pub fn log_stream_arn(region: &str, group_name: &str, stream_name: &str) -> String {
    ::engine::emulator_arn(
        "logs",
        region,
        &format!("log-group:{group_name}:log-stream:{stream_name}"),
    )
}

/// The service and resource segments of an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), or `None` for anything
/// that is not an ARN. The resource segment is whatever follows the account, colons and
/// slashes included, because a resource id carries its own separators.
pub fn arn_service_and_resource(arn: &str) -> Option<(&str, &str)> {
    arn_parts(arn).map(|(_partition, service, _account, resource)| (service, resource))
}

/// The account segment of an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), or `None` for anything that
/// is not an ARN or that names no account — which is how a refusal states the account the
/// identifier it was handed belongs to.
pub fn arn_account_id(arn: &str) -> Option<&str> {
    arn_parts(arn)
        .map(|(_partition, _service, account, _resource)| account)
        .filter(|account| !account.is_empty())
}

/// The `(partition, service, account, resource)` segments of an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), or `None` for anything
/// that is not an ARN.
fn arn_parts(arn: &str) -> Option<(&str, &str, &str, &str)> {
    let rest = arn.strip_prefix("arn:")?;
    // partition, service, region, account, then the whole resource segment.
    let mut segments = rest.splitn(5, ':');
    let partition = segments.next()?;
    let service = segments.next()?;
    let _region = segments.next()?;
    let account = segments.next()?;
    Some((
        partition,
        service,
        account,
        segments.next().unwrap_or_default(),
    ))
}

/// The vendors of a `destinationArn` CloudWatch Logs delivers to by ASSUMING the role
/// `roleArn` names, so a subscription against one of them cannot be configured without
/// that role: a stream of one of the two streaming services named as direct destinations
/// ("An Amazon Kinesis stream belonging to the same account as the subscription filter",
/// "A Kinesis Data Firehose delivery stream belonging to the same account").
///
/// The other two destination kinds carry their own authorisation, so neither needs
/// `roleArn`: a Lambda function is invoked directly, which is why the operation documents
/// the `iam:PassRole` requirement for "any destination except a Lambda function", and a
/// logical destination created with `PutDestination` was given the role it delivers
/// through when it was created (`roleArn`: "You don't need to provide the ARN when you are
/// working with a logical destination for cross-account delivery").
pub const ROLE_ASSUMED_DESTINATION_VENDORS: [&str; 2] = ["kinesis", "firehose"];

/// The vendor of a `destinationArn` whose delivery needs the role `roleArn` names, or
/// `None` when the filter can be configured without one.
///
/// Only the vendors in [`ROLE_ASSUMED_DESTINATION_VENDORS`] need one. A vendor this
/// operation delivers to nothing in needs no role either: there is no delivery to
/// authorise, and such a `destinationArn` is refused for the resource it names instead.
pub fn subscription_role_required_vendor(arn: &str) -> Option<&str> {
    let (_partition, vendor, _account, _resource) = arn_parts(arn)?;
    ROLE_ASSUMED_DESTINATION_VENDORS
        .contains(&vendor)
        .then_some(vendor)
}

/// Whether an ARN names a CloudWatch Logs log group — the only resource type a resource
/// policy can be attached to (`PutResourcePolicy$resourceArn`: "Currently only supports
/// LogGroup ARN").
///
/// A log group's ARN is `arn:<partition>:logs:<region>:<account>:log-group:<name>`, with
/// or without the trailing `:*` the `LogGroup$arn` member carries. A log STREAM's ARN
/// also names a log group, so it is excluded here: it names a stream, which cannot carry
/// a resource policy.
pub fn is_log_group_arn(arn: &str) -> bool {
    arn_service_and_resource(arn).is_some_and(|(service, resource)| {
        service == "logs"
            && resource.starts_with("log-group:")
            && !resource.contains(":log-stream:")
    })
}

/// The CloudWatch Logs resource one tagging operation's `resourceArn` names, resolved to
/// the key this service's store holds that resource under.
///
/// `TagResource` names the kinds that carry tags — "Currently, the only CloudWatch Logs
/// resources that can be tagged are log groups and destinations" — and `resourceArn`'s own
/// documentation gives both ARN forms: "The ARN format of a log group is
/// arn:aws:logs:Region:account-id:log-group:log-group-name The ARN format of a destination is
/// arn:aws:logs:Region:account-id:destination:destination-name". A delivery is here as well,
/// because the model gives that resource its own tag member (`Delivery$tags`: "The tags that
/// have been assigned to this delivery") and this service stores it
/// ([`DeliveryRecord::tags`]). The delivery source and the delivery destination are here for
/// the same reason: each carries its own tag member (`DeliverySource$tags`,
/// `DeliveryDestination$tags`) and this service stores it ([`DeliverySourceRecord::tags`],
/// [`DeliveryDestinationRecord::tags`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// One log group, by the name the store is keyed by.
    LogGroup(String),
    /// One logical destination, by the `Destination$destinationName` it is created under.
    Destination(String),
    /// One delivery, by the `Delivery$id` it was minted with in the Region that holds it.
    Delivery(String),
    /// One delivery source, by the `DeliverySource$name` it was created under in the Region
    /// that holds it.
    DeliverySource(String),
    /// One delivery destination, by the `DeliveryDestination$name` it was created under in the
    /// Region that holds it.
    DeliveryDestination(String),
    /// One lookup table, by the `LookupTable$lookupTableName` it was created under — the key
    /// the store holds it by. It is here for the same reason the delivery resources are: the
    /// model gives the resource its own tag member (`CreateLookupTable$tags`, "An optional list
    /// of key-value pairs to associate with the resource") and this service stores it
    /// ([`LookupTableRecord::tags`]).
    LookupTable(String),
}

impl TaggedResource {
    /// The resource kind a refusal names, in the words this service uses for it — what
    /// [`resource_not_found_message`] states when the ARN names no stored resource.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::LogGroup(_) => "log group",
            Self::Destination(_) => "destination",
            Self::Delivery(_) => "delivery",
            Self::DeliverySource(_) => "delivery source",
            Self::DeliveryDestination(_) => "delivery destination",
            Self::LookupTable(_) => "lookup table",
        }
    }
}

/// The ARN resource-segment prefix of a logical destination, from the ARN format
/// `TagResource$resourceArn` documents for one.
const DESTINATION_ARN_PREFIX: &str = "destination:";

/// The `Destination$destinationName` an ARN names — the name the store keys a logical
/// destination by — or `None` for an ARN that names no `destination` resource of this
/// service.
///
/// The ARN format is the one `TagResource$resourceArn` documents
/// (`arn:aws:logs:Region:account-id:destination:destination-name`), and the name is
/// whatever follows that resource prefix, separators included: `DestinationName` is
/// `^[^:*]*$`, so a name can carry slashes and dots.
pub fn destination_name_from_arn(arn: &str) -> Option<&str> {
    let (service, resource) = arn_service_and_resource(arn)?;
    if service != "logs" {
        return None;
    }
    resource
        .strip_prefix(DESTINATION_ARN_PREFIX)
        .filter(|name| !name.is_empty())
}

/// The ARN resource-segment prefix of a delivery, as [`delivery_arn`] builds one.
const DELIVERY_ARN_PREFIX: &str = "delivery:";

/// The ARN resource-segment prefix of a delivery source, as [`delivery_source_arn`] builds
/// one.
const DELIVERY_SOURCE_ARN_PREFIX: &str = "delivery-source:";

/// The ARN resource-segment prefix of a delivery destination, as
/// [`delivery_destination_arn`] builds one.
const DELIVERY_DESTINATION_ARN_PREFIX: &str = "delivery-destination:";

/// The resource a tagging operation's `resourceArn` names ([`TaggedResource`]), or `None`
/// for an ARN that names no CloudWatch Logs resource carrying tags — another service's
/// resource, or a CloudWatch Logs resource kind that holds no tag set (a log stream, say).
/// Whether the named resource EXISTS is a separate question, answered by
/// [`ComAmazonawsCloudwatchlogsLogs20140328State::resource_tags`].
pub fn tagged_resource(arn: &str) -> Option<TaggedResource> {
    let (service, resource) = arn_service_and_resource(arn)?;
    if service != "logs" {
        return None;
    }
    // Either log-group ARN form, with or without the trailing `:*` the `LogGroup$arn`
    // member carries, names the same group.
    if is_log_group_arn(arn) {
        return Some(TaggedResource::LogGroup(
            log_group_name_from_identifier(arn).to_string(),
        ));
    }
    if let Some(name) = destination_name_from_arn(arn) {
        return Some(TaggedResource::Destination(name.to_string()));
    }
    if let Some(name) = resource.strip_prefix(DELIVERY_SOURCE_ARN_PREFIX) {
        return (!name.is_empty()).then(|| TaggedResource::DeliverySource(name.to_string()));
    }
    if let Some(name) = resource.strip_prefix(DELIVERY_DESTINATION_ARN_PREFIX) {
        return (!name.is_empty()).then(|| TaggedResource::DeliveryDestination(name.to_string()));
    }
    if let Some(id) = resource.strip_prefix(DELIVERY_ARN_PREFIX) {
        return (!id.is_empty()).then(|| TaggedResource::Delivery(id.to_string()));
    }
    if let Some(name) = lookup_table_name_from_arn(arn) {
        return Some(TaggedResource::LookupTable(name.to_string()));
    }
    None
}

/// The refusal for a `resourceArn` that is a well-formed ARN but names no resource this
/// service can tag, stated as `TagResource` states the rule: "Currently, the only CloudWatch
/// Logs resources that can be tagged are log groups and destinations." A value that is not
/// an ARN at all is a different refusal ([`unparsable_arn_message`]), and a taggable kind
/// the account holds no instance of is a third (`ResourceNotFoundException`).
pub const UNTAGGABLE_RESOURCE_ARN: &str =
    "resourceArn must name a resource that supports tagging. Currently, the only CloudWatch \
     Logs resources that can be tagged are log groups and destinations.";

/// Whether an ARN names the account's stored query-insights results — the non-log-group
/// target `AssociateKmsKey` / `DisassociateKmsKey` document: "Specify the following ARN
/// to have future GetQueryResults operations in this account encrypt the results with
/// the specified KMS key [...] arn:aws:logs:REGION:ACCOUNT_ID:query-result:*".
pub fn is_query_result_arn(arn: &str) -> bool {
    arn_service_and_resource(arn)
        .is_some_and(|(service, resource)| service == "logs" && resource == "query-result:*")
}

/// The ARN of the account's stored query-insights results, the one value
/// [`is_query_result_arn`] admits: "arn:aws:logs:REGION:ACCOUNT_ID:query-result:*".
pub fn query_results_arn(region: &str) -> String {
    ::engine::emulator_arn("logs", region, "query-result:*")
}

/// The refusal for a member this service parses as a KMS key's and was handed a value that
/// names something else: the message names the MEMBER and the value it would not take, so a
/// caller can fix the request from it. Shared by every operation that holds a member to
/// naming a KMS key — `kmsKeyId` of the lookup-table writes and `kmsKeyArn` of
/// `PutIntegration` — so the wording cannot drift between them.
pub fn not_a_kms_key_message(member: &str, value: &str) -> String {
    format!("{member} must name a KMS key, got '{value}'")
}

/// Whether a `kmsKeyId` names a KMS key: the member is documented as "The Amazon
/// Resource Name (ARN) of the KMS key to use when encrypting log data", so a value that
/// is an ARN must be one of KMS's. A bare key id or alias carries no service segment and
/// is left to KMS to resolve, but an empty value names nothing at all.
pub fn is_kms_key_id(kms_key_id: &str) -> bool {
    if kms_key_id.trim().is_empty() {
        return false;
    }
    match arn_service_and_resource(kms_key_id) {
        Some((service, resource)) => service == "kms" && !resource.is_empty(),
        None => !kms_key_id.starts_with("arn:"),
    }
}

/// Whether an ARN names a destination `PutSubscriptionFilter` supports delivering to:
/// one of the services in [`SUBSCRIPTION_DESTINATIONS`], naming a resource of the kind
/// that service's entry there gives.
///
/// A CloudWatch Logs ARN qualifies only when it names a logical `destination` (the
/// resource `PutDestination` creates), never a log group or a log stream.
pub fn is_subscription_destination_arn(arn: &str) -> bool {
    arn_service_and_resource(arn).is_some_and(|(service, resource)| {
        SUBSCRIPTION_DESTINATIONS
            .iter()
            .any(|(name, prefix)| service == *name && resource.starts_with(prefix))
    })
}

/// AWS's request-derived refusal message for a member the service parses as an ARN and
/// was handed something that is not one: the validation-error template naming the member
/// and the value it could not parse. `None` when the value parses as an ARN, whatever
/// resource it names — a well-formed ARN of the wrong kind is a different refusal, with
/// its own message.
pub fn unparsable_arn_message(member: &str, value: &str) -> Option<String> {
    arn_service_and_resource(value)
        .is_none()
        .then(|| format!("1 validation error detected: Unable to parse {member} {value}"))
}

/// Whether `value` matches a term carrying `*` wildcards, each standing for any run of
/// characters (`prod*`, `*prod`, `a*b*c`); a term with no `*` matches only itself.
///
/// THE one reading of the `*` wildcard this service uses, shared by every member that
/// carries one: a `ListLogGroups` name term, and a filter pattern's `"*…*"` literal
/// ([`filter_pattern`]). Case folding is the CALLER's, because the two differ there — log
/// group name matching is documented case-INsensitive, and the filter-pattern syntax is case
/// sensitive — so this answers only the wildcard question.
pub fn wildcard_matches(value: &str, term: &str) -> bool {
    let mut literals = term.split('*');
    // The run before the first `*` is anchored at the start, the run after the last `*`
    // at the end, and every run between them is matched in order.
    let Some(first) = literals.next() else {
        return true;
    };
    let Some(mut rest) = value.strip_prefix(first) else {
        return false;
    };
    let remaining: Vec<&str> = literals.collect();
    let Some((last, middle)) = remaining.split_last() else {
        // No `*` at all: the term is literal.
        return rest.is_empty();
    };
    for literal in middle {
        match rest.find(literal) {
            Some(at) => rest = &rest[at + literal.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// Hold a `filterPattern` to the filter-pattern syntax's bracket rule: a pattern that
/// opens one of the bracketed forms in [`FILTER_PATTERN_DELIMITERS`] must close it with
/// the matching delimiter, and the brackets it nests inside that form must balance. The
/// `Err` carries the message to refuse with, as the service states the rule.
///
/// A pattern that opens no bracketed form — a term, a text or a regular-expression
/// pattern — carries no bracket structure to check, and the empty pattern (the modelled
/// `FilterPattern` is `length 0..1024`) matches every event.
pub fn validate_filter_pattern(pattern: &str) -> Result<(), String> {
    let pattern = pattern.trim();
    let Some(first) = pattern.chars().next() else {
        return Ok(());
    };
    let Some((open, close)) = FILTER_PATTERN_DELIMITERS
        .iter()
        .copied()
        .find(|(open, _)| *open == first)
    else {
        return Ok(());
    };
    if !pattern.ends_with(close) {
        return Err(format!(
            "If a filter pattern starts with '{open}' it must end with '{close}'"
        ));
    }
    // Every structural defect is refused with the one fixed text the service answers for a
    // pattern it cannot read ([`filter_pattern::INVALID_PATTERN`]): which of the three the
    // pattern has is not part of that message, so the reading below is kept as the check it
    // is and nothing of it is reported.
    if filter_pattern_structure(pattern, open).is_err() {
        return Err(filter_pattern::INVALID_PATTERN.to_string());
    }
    Ok(())
}

/// What [`filter_pattern_structure`] found wrong with a pattern, so the refusal names the
/// defect it detected rather than one of them for all three.
enum PatternDefect {
    UnbalancedBrackets,
    UnterminatedQuotedString,
    UnterminatedRegex,
}

/// Whether a filter pattern's structure closes: its brackets open and close in order —
/// the bracketed forms in [`FILTER_PATTERN_DELIMITERS`], plus the `()` that group a JSON
/// pattern's terms, only in a JSON pattern, because a space-delimited pattern's terms can
/// carry a bare parenthesis — and the literals it opens are terminated.
///
/// A quoted string (`"…"`) and a regular-expression literal (`%…%`) are opaque: the
/// pattern syntax lets them carry any character, so a bracket inside one is data rather
/// than structure. A backslash escapes the character after it.
fn filter_pattern_structure(pattern: &str, opened: char) -> Result<(), PatternDefect> {
    let groups_terms_in_parentheses = opened == '{';
    let mut open_brackets: Vec<char> = Vec::new();
    let mut quoted = false;
    let mut regex = false;
    let mut chars = pattern.chars();
    while let Some(character) = chars.next() {
        match character {
            '\\' => {
                chars.next();
            }
            '"' if !regex => quoted = !quoted,
            '%' if !quoted => regex = !regex,
            _ if quoted || regex => {}
            '(' | ')' if !groups_terms_in_parentheses => {}
            '{' | '[' | '(' => open_brackets.push(character),
            '}' | ']' | ')' => {
                let opened = match character {
                    '}' => '{',
                    ']' => '[',
                    _ => '(',
                };
                if open_brackets.pop() != Some(opened) {
                    return Err(PatternDefect::UnbalancedBrackets);
                }
            }
            _ => {}
        }
    }
    if quoted {
        return Err(PatternDefect::UnterminatedQuotedString);
    }
    if regex {
        return Err(PatternDefect::UnterminatedRegex);
    }
    if open_brackets.is_empty() {
        Ok(())
    } else {
        Err(PatternDefect::UnbalancedBrackets)
    }
}

/// The scheduled query name a `ScheduledQueryIdentifier` names: the identifier itself when it
/// is a name, or the name segment of a scheduled-query ARN ([`scheduled_query_arn`]).
///
/// A scheduled query is identified by "The ARN or name of the scheduled query", and the ARN
/// is built FROM the name, so an ARN names the query its resource segment does — the way a
/// `logGroupIdentifier` names a log group through [`log_group_name_from_identifier`].
pub fn scheduled_query_name_from_identifier(identifier: &str) -> &str {
    const MARKER: &str = ":scheduled-query:";
    match identifier.find(MARKER) {
        Some(at) => &identifier[at + MARKER.len()..],
        None => identifier,
    }
}

/// The log group name a `logGroupIdentifier` names: the identifier itself when it is a
/// name, or the name segment of a log-group ARN (either ARN version).
pub fn log_group_name_from_identifier(identifier: &str) -> &str {
    const MARKER: &str = ":log-group:";
    match identifier.find(MARKER) {
        Some(at) => {
            let rest = &identifier[at + MARKER.len()..];
            let rest = rest.strip_suffix(":*").unwrap_or(rest);
            // A log-stream ARN carries the group name before `:log-stream:`.
            match rest.find(":log-stream:") {
                Some(end) => &rest[..end],
                None => rest,
            }
        }
        None => identifier,
    }
}

/// The refusal for a request that names BOTH log-group selectors, and the one that names
/// NEITHER ([`resolve_log_group_target`]).
///
/// This pair's refusal is the one CloudWatch Logs answers with `ValidationException`
/// (HTTP 400) rather than with the operation's own `InvalidParameterException`: it is
/// checked across two members, which no single member's constraint can express, and the
/// recorded refusal for a request naming both selectors is `400 ValidationException:
/// LogGroup name and LogGroup ARN are mutually exclusive parameters.` — which is why each
/// caller widens that shape in through `extra_errors`. The two selectors are named here, the
/// way the service names them — the log group's NAME and its ARN — and the SENTENCE is the
/// service's one exclusive-pair form ([`parameter_pairs::mutually_exclusive`]).
pub static LOG_GROUP_SELECTORS_EXCLUSIVE: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| parameter_pairs::mutually_exclusive("LogGroup name", "LogGroup ARN"));

/// The refusal for a request that names neither selector; see
/// [`LOG_GROUP_SELECTORS_EXCLUSIVE`]. Both sentences are the service's own fixed wording
/// for a request-independent refusal, and both name the selectors as the service names
/// them rather than as the members spell them.
pub static LOG_GROUP_SELECTOR_REQUIRED: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| parameter_pairs::either_required("LogGroupName", "LogGroupArn"));

/// Resolve the log group an operation's `logGroupName` / `logGroupIdentifier` pair
/// names. The model documents them as mutually exclusive with one of them required, so
/// neither-or-both is a client error; the `Err` carries the message to refuse with, and
/// [`LOG_GROUP_SELECTORS_EXCLUSIVE`] records which error shape a caller refuses with.
pub fn resolve_log_group_target(
    log_group_name: Option<&str>,
    log_group_identifier: Option<&str>,
) -> Result<String, String> {
    match (log_group_name, log_group_identifier) {
        (Some(name), None) => Ok(name.to_string()),
        (None, Some(identifier)) => Ok(log_group_name_from_identifier(identifier).to_string()),
        (Some(_), Some(_)) => Err(LOG_GROUP_SELECTORS_EXCLUSIVE.clone()),
        (None, None) => Err(LOG_GROUP_SELECTOR_REQUIRED.clone()),
    }
}

/// What an `AssociateKmsKey` / `DisassociateKmsKey` request names: "you specify either
/// the logGroupName parameter or the resourceIdentifier parameter", and a
/// `resourceIdentifier` is either a log group's ARN or the account's query-result ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KmsKeyTarget {
    /// One log group, by the name the store is keyed by.
    LogGroup(String),
    /// The account's stored query-insights results.
    QueryResults,
}

/// The sentence an `AssociateKmsKey` / `DisassociateKmsKey` request naming BOTH of its two
/// target members is refused with: `resourceIdentifier` is the member that identifies the
/// target on its own, so it is the member the sentence names. Fixed text for the class,
/// carrying nothing of the request.
pub const RESOURCE_IDENTIFIER_EXCLUSIVE_MESSAGE: &str =
    "resourceIdentifier should not be specified with any other log group attributes";

/// The refusal an `AssociateKmsKey` / `DisassociateKmsKey` request whose two target members
/// cannot hold together is refused with, or `None` when exactly one of them names a target.
///
/// A request naming both, or neither, breaks the rule that RELATES the two members ("you
/// specify either the logGroupName parameter or the resourceIdentifier parameter. You can't
/// specify both of those parameters in the same operation") rather than any constraint on a
/// value, so it is answered as the request-validation refusal it is — a `ValidationException`,
/// like every other mutually-exclusive pair of a log-group selector this service checks — and
/// not as one of the operation's own modelled parameter errors. A `resourceIdentifier` that
/// names no resource IS a bad value, and stays one ([`resolve_kms_key_target`]).
pub fn kms_key_target_pair_refusal(
    log_group_name: Option<&str>,
    resource_identifier: Option<&str>,
) -> Option<String> {
    match (log_group_name, resource_identifier) {
        (Some(_), None) | (None, Some(_)) => None,
        // The two members this rule relates, named as the service names them: the one that
        // identifies a target on its own is the one the sentence is about.
        (Some(_), Some(_)) => Some(RESOURCE_IDENTIFIER_EXCLUSIVE_MESSAGE.to_string()),
        (None, None) => Some(parameter_pairs::either_required(
            "logGroupName",
            "resourceIdentifier",
        )),
    }
}

/// Resolve the target of an `AssociateKmsKey` / `DisassociateKmsKey` request. The two
/// parameters are documented as mutually exclusive with one of them required, and a
/// `resourceIdentifier` naming neither a log group nor the query-result resource names
/// no target at all; the `Err` carries the message to refuse with.
pub fn resolve_kms_key_target(
    log_group_name: Option<&str>,
    resource_identifier: Option<&str>,
) -> Result<KmsKeyTarget, String> {
    match (log_group_name, resource_identifier) {
        (Some(name), None) => Ok(KmsKeyTarget::LogGroup(name.to_string())),
        (None, Some(identifier)) => {
            if is_query_result_arn(identifier) {
                Ok(KmsKeyTarget::QueryResults)
            } else if is_log_group_arn(identifier) {
                Ok(KmsKeyTarget::LogGroup(
                    log_group_name_from_identifier(identifier).to_string(),
                ))
            } else {
                Err(format!(
                    "resourceIdentifier must be a log group ARN or a query-result ARN, got '{identifier}'"
                ))
            }
        }
        // The same pair forms as every other two-member check of this service
        // ([`parameter_pairs`]), over the two members this one relates — which the
        // operation's own documentation names in their member spellings ("you specify
        // either the logGroupName parameter or the resourceIdentifier parameter").
        (Some(_), Some(_)) => Err(parameter_pairs::mutually_exclusive(
            "logGroupName",
            "resourceIdentifier",
        )),
        (None, None) => Err(parameter_pairs::either_required(
            "logGroupName",
            "resourceIdentifier",
        )),
    }
}

/// How many milliseconds one second holds, which is what a `StartQuery` time range — "epoch
/// time, the number of SECONDS since January 1, 1970" — has to be scaled by to be compared
/// with the epoch MILLISECONDS every other timestamp of this service carries.
pub const MILLIS_PER_SECOND: i64 = 1_000;

/// The name the SDK envelope this service's refusals carry names the service by — the
/// legacy client name of CloudWatch Logs, not the model's `sdkId`.
const SDK_ENVELOPE_SERVICE: &str = "AWSLogs";

/// The HTTP status and error code of one refusal, kept PAIRED so neither can be stated in
/// an SDK envelope without the other: they are the two values a client reads back out of
/// the message text, and they must be the status and code the response itself carries.
pub type SdkEnvelopeFacts = (u16, &'static str);

/// The status and code of a refusal for a parameter "specified incorrectly", as the model
/// declares `InvalidParameterException` for this service (client error, HTTP 400).
pub const ENVELOPE_INVALID_PARAMETER: SdkEnvelopeFacts = (400, "InvalidParameterException");

/// The status and code of a refusal for a resource that "does not exist", as the model
/// declares `ResourceNotFoundException` for this service (client error, HTTP 400).
pub const ENVELOPE_RESOURCE_NOT_FOUND: SdkEnvelopeFacts = (400, "ResourceNotFoundException");

/// `body` followed by the envelope the AWS SDK appends to a CloudWatch Logs refusal:
/// ` (Service: AWSLogs; Status Code: <status>; Error Code: <code>; Request ID: <id>; Proxy: null)`.
///
/// `facts` is the status-and-code pair of the refusal being worded (one of the
/// `ENVELOPE_*` constants). The request id is the ONLY per-run field: a request id is
/// minted by the service per call — nothing in the request carries one — so it comes from
/// the emulator's one request-id mint
/// ([`service_common::query_error_envelope::new_request_id`]), a fresh v4 UUID
/// indistinguishable from the one the framework's own renderers report. Nothing of the
/// request is interpolated.
///
/// One function, so every refusal worded this way carries the envelope in the same shape.
pub fn with_sdk_error_envelope(body: &str, facts: SdkEnvelopeFacts) -> String {
    let (status_code, error_code) = facts;
    format!(
        "{body} (Service: {SDK_ENVELOPE_SERVICE}; Status Code: {status_code}; \
         Error Code: {error_code}; Request ID: {}; Proxy: null)",
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// The refusal every paginated read of this service answers for a `nextToken` it cannot
/// resume the enumeration from — one this service never minted, one whose payload it cannot
/// decode, and one minted for a different enumeration than the request carrying it describes.
/// AWS states one fixed sentence for the whole class, carrying neither the token nor the
/// operation, so a paginating reader that keys on it restarts from the head of the stream.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "The specified nextToken is invalid.";

/// The refusal `CreateExportTask` answers when the S3 bucket its `destination` names is one
/// the object store does not hold: an export that could never write anything mints no task.
/// Fixed text for the class — it carries neither the bucket name nor anything else of the
/// request.
pub const EXPORT_DESTINATION_BUCKET_MISSING_MESSAGE: &str =
    "The given bucket does not exist. Please make sure the bucket is valid.";

/// The sentence CloudWatch Logs answers when the time range a query is asked over runs
/// backwards — its end before its start — which includes no instant at all. Fixed text for
/// the condition: it names the two ends of the range and interpolates neither of their
/// values.
pub const REVERSED_TIME_RANGE_MESSAGE: &str = "End time cannot be less than Start time";

/// The whole message such a refusal carries: the fixed sentence plus the SDK envelope of
/// the `InvalidParameterException` it is refused under, so the status and code stated inside
/// the text are the ones the response answers with.
pub fn reversed_time_range_refusal() -> String {
    with_sdk_error_envelope(REVERSED_TIME_RANGE_MESSAGE, ENVELOPE_INVALID_PARAMETER)
}

/// The refusal `CreateExportTask` answers when the window it is given is not a time range:
/// its fixed text states what the service requires of a window — that the two values bound a
/// period the log groups still hold data for, and that `from` precede `to` — and its two
/// quoted slots carry the window the REQUEST sent, in the order it sent it, so the caller can
/// see which window was rejected.
pub fn export_time_range_message(from: i64, to: i64) -> String {
    format!(
        "Specified time range of '{from}' and '{to}' is not valid. Please make sure the values \
         are within the retention period of the log groups and from value is lesser than the to \
         value"
    )
}

/// The refusal `CancelExportTask` answers when the task the request names is not in one of
/// the two statuses a cancel applies to: "The task must be in the PENDING or RUNNING state."
/// The text is fixed for the condition — it interpolates neither the task id, nor the log
/// group, nor the bucket the export was going to — and the task keeps the status it reached.
pub const EXPORT_TASK_FINISHED_MESSAGE: &str = "The specified export task has already finished";

/// The revision id a resource-scoped policy carries, minted from the monotonic policy
/// revision counter so it is unique and derived from stored state. AWS documents no
/// format for it — only that a put must present the revision the policy is currently
/// at — so this renders the counter as a fixed-width hex string.
pub fn mint_revision_id(seq: u64) -> String {
    format!("{seq:032x}")
}

/// How many characters a minted revision id has, which is the width
/// [`mint_revision_id`] renders the counter at. A presented `expectedRevisionId` of any
/// other length is in no form this service ever issued
/// ([`ComAmazonawsCloudwatchlogsLogs20140328State::issued_revision_id`]).
pub const REVISION_ID_LEN: usize = 32;

/// The refusal CloudWatch Logs answers a resource-policy write or delete with when the
/// `expectedRevisionId` it presents is not a revision id the service ever issued — a value
/// in another form, or one no update has reached. That is a bad PARAMETER and not the
/// optimistic-lock conflict [`POLICY_REVISION_CONFLICT_MESSAGE`] reports: retrying it can
/// never succeed, so the text names the member at fault and interpolates nothing.
pub const INVALID_EXPECTED_REVISION_ID_MESSAGE: &str = "Invalid ExpectedRevisionId";

/// The sequence token a `PutLogEvents` call hands back, minted from the monotonic
/// ingestion counter so it is unique and derived from stored state. Rendered the way
/// CloudWatch Logs renders one: a fixed-width decimal string.
pub fn mint_sequence_token(seq: u64) -> String {
    format!("{seq:056}")
}

/// A canonical hyphenated UUID minted from one of the store's monotonic counters, which
/// is how every resource of this service whose id AWS renders as a UUID gets one: unique
/// and DERIVED FROM STORED STATE rather than from a random source this crate would need a
/// new dependency for — the way [`mint_revision_id`] and [`mint_sequence_token`] mint from
/// stored counters. The version (`4`) and variant (`8`) nibbles sit where a UUID carries
/// them, and the counter fills the node field, offset by one so the first id minted is not
/// the nil UUID.
pub fn mint_uuid_from_seq(seq: u64) -> String {
    format!("00000000-0000-4000-8000-{:012x}", seq.saturating_add(1))
}

/// The id `PutQueryDefinition` hands back for a definition it creates: "CloudWatch
/// generates a unique ID for the new query definition and include it in the response to
/// this operation."
///
/// CloudWatch Logs renders one as a canonical hyphenated UUID, so it is minted through the
/// shared [`mint_uuid_from_seq`] from the store's monotonic query-definition counter.
pub fn mint_query_definition_id(seq: u64) -> String {
    mint_uuid_from_seq(seq)
}

/// The id `StartQuery` hands back for the query it starts: "The unique ID of the query",
/// minted from the store's monotonic query counter so it is unique for the life of the store
/// and DERIVED FROM STORED STATE.
///
/// CloudWatch Logs renders one as a canonical hyphenated UUID, so it is minted through the
/// shared [`mint_uuid_from_seq`] every UUID-shaped id of this service comes from.
pub fn mint_query_id(seq: u64) -> String {
    mint_uuid_from_seq(seq)
}

/// The `S3TableIntegrationSourceIdentifier` `AssociateSourceToS3TableIntegration` hands
/// back for the association it creates: "The unique identifier for the association between
/// the data source and S3 Table Integration", minted from the store's monotonic association
/// counter so it is unique for the life of the store and DERIVED FROM STORED STATE.
///
/// Minted through the shared [`mint_uuid_from_seq`] every id of this service AWS renders as
/// a UUID comes from, which also makes the minting order the identifier order the
/// associations are stored and enumerated in.
pub fn mint_s3_table_integration_source_identifier(seq: u64) -> String {
    mint_uuid_from_seq(seq)
}

/// The refusal CloudWatch Logs answers when the resource an operation names is not
/// there. The service states the resource KIND that is missing, in the sentence form the
/// model documents for `ResourceNotFoundException` — "The specified resource does not
/// exist." — terminated like that sentence, so every operation of this service refuses a
/// missing resource the same way.
pub fn resource_not_found_message(resource_kind: &str) -> String {
    format!("The specified {resource_kind} does not exist.")
}

/// The refusal CloudWatch Logs answers when the missing resource is a QUERY the request
/// named by `queryId`: the service states the id it was handed rather than the resource kind,
/// and calls it INVALID rather than absent — a caller holding several ids learns which one was
/// rejected, and keeps the distinction the service draws between an id it cannot place and a
/// query that is simply no longer running ([`query_not_running_message`]).
///
/// The only variable part is the `queryId` the request carried, taken from it verbatim.
///
/// This is the sentence `StopQuery` refuses such an id with. `GetQueryResults` words its own
/// ([`query_results_invalid_query_id_message`]) and refuses it under the other error class the
/// two operations both model, so a caller holding one handle can tell WHICH read refused it
/// and the two templates are not unified.
pub fn invalid_query_id_message(query_id: &str) -> String {
    format!("The provided queryId={query_id} is invalid.")
}

/// The refusal `GetQueryResults` answers for a `queryId` that names no query of this account.
///
/// The operation models both `InvalidParameterException` ("A parameter is specified
/// incorrectly") and `ResourceNotFoundException` ("The specified resource does not exist"), and
/// CloudWatch Logs refuses an unissued results handle as the incorrectly specified PARAMETER of
/// the two: a handle that was never valid, rather than a resource that was there and is gone.
/// The sentence is this operation's own, spaced and terminated as the service words it, and its
/// only variable part is the `queryId` the request carried, taken from it verbatim.
pub fn query_results_invalid_query_id_message(query_id: &str) -> String {
    format!("The provided queryId = {query_id} is invalid ")
}

/// The refusal `GetQueryResults` answers for a `nextToken` it cannot resume THIS query's
/// results from — a token minted for another query, and one whose payload it cannot decode at
/// all.
///
/// The fixed text is the sentence this operation words that condition with, stated inside the
/// SDK envelope of the `InvalidParameterException` it is refused under
/// ([`with_sdk_error_envelope`]), so the status and the code stated in the text are the ones the
/// response itself answers with and the only per-run field is the request id.
pub fn query_results_invalid_token_message() -> String {
    with_sdk_error_envelope("Invalid token for query", ENVELOPE_INVALID_PARAMETER)
}

/// The refusal CloudWatch Logs answers when the missing resource is a QUERY DEFINITION: the
/// service states the fixed sentence for the class — it carries nothing of the request, not
/// even the id — inside the SDK envelope of the `ResourceNotFoundException` it is refused
/// under ([`with_sdk_error_envelope`]), so the status and the code stated in the text are the
/// ones the response itself answers with and the only per-run field is the request id.
pub fn query_definition_not_found_message() -> String {
    with_sdk_error_envelope(
        "Query definition id is invalid.",
        ENVELOPE_RESOURCE_NOT_FOUND,
    )
}

/// The refusal CloudWatch Logs answers when the missing resource is a SCHEDULED QUERY: the
/// service names the `ScheduledQueryIdentifier` the request asked for ("The ARN or name of the
/// scheduled query") rather than stating the resource kind alone, and states it unterminated,
/// so a caller reading the refusal sees WHICH query was not found — whichever form it named it
/// by.
///
/// Every operation that resolves a `ScheduledQueryIdentifier` refuses a query the account does
/// not hold through this one sentence, so the reads and the writes of a scheduled query agree.
pub fn scheduled_query_not_found_message(identifier: &str) -> String {
    format!("Scheduled query with identifier {identifier} not found")
}

/// The refusal CloudWatch Logs answers a scheduled-query create with when the account already
/// holds a query under the `name` it asks for ("The name must be unique within your account and
/// region", refused as the conflict the operation models: "This operation attempted to create a
/// resource that already exists").
///
/// It is the same identifier phrasing the service refuses a scheduled query it cannot find with
/// ([`scheduled_query_not_found_message`]): the resource kind, then the identifier the request
/// carried, then the condition — so the only variable part is that identifier, taken from the
/// request verbatim and carrying nothing the account generated (no ARN, account id or instant).
pub fn scheduled_query_already_exists_message(identifier: &str) -> String {
    format!("Scheduled query with identifier {identifier} already exists")
}

/// The refusal CloudWatch Logs answers when the missing resource is one the request
/// IDENTIFIES NOTHING OF: a log-group level policy document addressed by the identifier of
/// the log group it is attached to and by nothing else, which that group does not hold. The
/// service says nothing about the resource in that case — the text is fixed for the error
/// class and interpolates neither a log group, nor a name, nor a resource kind — unlike a
/// request naming a log group or a log stream the account does not hold, which is refused
/// by [`resource_not_found_message`] naming that resource, and unlike a policy the request
/// names, which is refused by [`policy_not_found_message`] carrying that name.
pub const UNIDENTIFIED_RESOURCE_NOT_FOUND_MESSAGE: &str = "Specified resource does not exist!";

/// The refusal CloudWatch Logs answers a resource-policy write or delete with when the
/// `expectedRevisionId` it presents is not the policy's current revision: the revision the
/// caller held was superseded, which the service reports as a conflict against the resource
/// that the caller is to re-read and retry. The text is fixed for the error class — it
/// interpolates neither the revision presented, nor the stored one, nor the policy's name
/// or resource.
pub const POLICY_REVISION_CONFLICT_MESSAGE: &str =
    "A conflicting operation is currently in progress against this resource. Please try again.";

/// The refusal CloudWatch Logs answers a delivery-destination POLICY operation with when the
/// `deliveryDestinationName` it names is not one this account holds. The delivery destination
/// policy operations state that the requested destination is not in the account rather than
/// the generic [`resource_not_found_message`] sentence, and they say it per NAME rather than
/// per request value, so every name the account does not hold is refused identically.
pub const DELIVERY_DESTINATION_NOT_FOUND_MESSAGE: &str =
    "Requested Delivery Destination does not exist in this account.";

/// The refusal CloudWatch Logs answers the DELETE of a delivery-destination policy with when
/// the destination it names holds no policy: the resource the delete addresses is the POLICY,
/// not the destination, so a destination that exists and has had none assigned has nothing for
/// the delete to remove. Like [`DELIVERY_DESTINATION_NOT_FOUND_MESSAGE`] the sentence is fixed
/// for the error class and interpolates no destination name.
pub const DELIVERY_DESTINATION_POLICY_NOT_FOUND_MESSAGE: &str =
    "Delivery Destination Policy does not exist for this Delivery Destination.";

/// The refusal `PutDeliveryDestinationPolicy` answers a `deliveryDestinationPolicy` that is
/// not a delivery destination resource policy ([`is_delivery_destination_policy`]). The
/// sentence is fixed for the error class: it states neither the destination nor anything of
/// the document it would not take.
pub const INVALID_DELIVERY_DESTINATION_POLICY_MESSAGE: &str =
    "Invalid Delivery Destination resource Policy is provided.";

/// The spelling the data protection half of this service gives the fixed missing-resource
/// sentence: the same sentence as [`UNIDENTIFIED_RESOURCE_NOT_FOUND_MESSAGE`], with the
/// verb inflected the way the data protection path words it. The text is fixed for the
/// class and carries nothing of the request.
pub const UNIDENTIFIED_DATA_PROTECTION_RESOURCE_NOT_FOUND_MESSAGE: &str =
    "Specified resource does not exists!";

/// The refusal `DeleteAccountPolicy` answers for a `(policyType, policyName)` pair the
/// account does not hold.
///
/// The account-level policy a delete addresses is identified by the two members of the
/// request and by nothing else, so — like every other resource this service identifies
/// nothing of — it is refused with the fixed missing-resource sentence, which interpolates
/// neither the name nor the type. The one variation is the SPELLING: the delete of a data
/// protection policy is served by the same path the log-group-level data protection
/// operations are served by, which inflects that sentence its own way
/// ([`UNIDENTIFIED_DATA_PROTECTION_RESOURCE_NOT_FOUND_MESSAGE`]), while every other policy
/// type answers [`UNIDENTIFIED_RESOURCE_NOT_FOUND_MESSAGE`].
pub fn account_policy_not_found_message(policy_type: &str) -> &'static str {
    if policy_type == POLICY_TYPE_DATA_PROTECTION {
        UNIDENTIFIED_DATA_PROTECTION_RESOURCE_NOT_FOUND_MESSAGE
    } else {
        UNIDENTIFIED_RESOURCE_NOT_FOUND_MESSAGE
    }
}

/// The refusal CloudWatch Logs answers when the `integrationName` an operation resolves is
/// not an integration this account holds. The fixed text states the kind of resource and
/// the condition around ONE variable part — the requested name, taken from the request
/// verbatim — so a caller managing several integrations can attribute the refusal to one of
/// them instead of guessing which of its requests failed.
pub fn integration_not_found_message(integration_name: &str) -> String {
    format!("Integration with name {integration_name} does not exist.")
}

/// The refusal CloudWatch Logs answers when the `anomalyDetectorArn` an operation resolves
/// is not a log anomaly detector this account holds.
///
/// The service reports the ARN it was handed rather than the kind of resource, and keeps the
/// ambiguity it draws deliberately: an ARN nothing ever minted and a detector a previous
/// delete already removed are refused identically, so the sentence states both
/// possibilities. The only variable part is that ARN, taken from the request verbatim.
pub fn anomaly_detector_not_found_message(anomaly_detector_arn: &str) -> String {
    format!("The specified ARN does not exist or already deleted : {anomaly_detector_arn}")
}

/// The refusal `DeleteTransformer` answers when the log group it names holds no transformer.
///
/// The transformer a delete addresses is identified by the log group it belongs to, so the
/// sentence states the kind of resource that is missing and the group it was looked for on —
/// the group the request named, resolved from its `logGroupIdentifier` — rather than the
/// fixed text a resource the request identifies nothing of is refused with.
pub fn transformer_not_found_message(log_group_name: &str) -> String {
    format!("Transformer does not exist on log group '{log_group_name}'")
}

/// The refusal the data protection policy READ answers when the `logGroupIdentifier` it
/// resolves names a log group the account does not hold.
///
/// A data protection policy is readable across accounts, so this refusal states both halves
/// of what was looked for: the group name the request asked for, and the account that would
/// own it — which is what makes the sentence usable when the group exists in another
/// account. Both variable parts are taken from the request and from the emulator's own
/// account identity; the sentence is otherwise fixed.
pub fn log_group_not_found_for_owner_message(log_group_name: &str, owner_account_id: &str) -> String {
    format!("Log group '{log_group_name}' for owner '{owner_account_id}' cannot be found")
}

/// The refusal CloudWatch Logs builds from the request when the identifier a
/// `DisassociateSourceFromS3TableIntegration` names is not an association of an S3 Table
/// Integration this account holds. The message interpolates the requested identifier, so a
/// caller disassociating sources in a loop can attribute the refusal to one identifier
/// instead of guessing which of its requests failed.
pub fn s3_table_integration_source_not_found_message(identifier: &str) -> String {
    format!("Association not found for identifier: {identifier}")
}

/// The refusal CloudWatch Logs builds from the request when the `destinationName` it names
/// is not a destination this account holds and the operation models no
/// `ResourceNotFoundException`, so the missing destination is reported through
/// `InvalidParameterException` — an error class documented only as "A parameter is specified
/// incorrectly", which carries no sentence of its own, so the message states WHICH parameter
/// value was rejected. The fixed text is "Destination with name " … " does not exist" and the
/// one variable part is the requested name, taken verbatim from the request, so a caller
/// managing several destinations can attribute the refusal to one request instead of guessing
/// which of them failed.
///
/// Unlike this, an operation that DOES model `ResourceNotFoundException` refuses a missing
/// destination with that error class's own documented sentence ("The specified resource does
/// not exist."), specialised per resource kind by [`resource_not_found_message`].
pub fn destination_not_found_message(destination_name: &str) -> String {
    format!("Destination with name {destination_name} does not exist")
}

/// The refusal CloudWatch Logs builds from the request when the resource policy the
/// caller named is not there. The message interpolates the requested `policyName`, so a
/// caller deleting policies in a loop can attribute the refusal to one policy name
/// instead of guessing which of its requests failed.
pub fn policy_not_found_message(policy_name: &str) -> String {
    format!("Policy with name [{policy_name}] does not exist.")
}

/// The refusal for a value outside the set a member's own documentation enumerates: the
/// value is declared invalid and the permitted set is then listed, so the refusal itself
/// tells a caller which values it may use.
///
/// `subject` names what was invalid (the "retention" of `retentionInDays`, say) and
/// `valid` is the catalogue the model states — [`RETENTION_IN_DAYS_VALUES`] or
/// [`SUBSCRIPTION_SYSTEM_FIELDS`] — rendered in the documented order, never a list typed
/// into a message.
pub fn invalid_value_message<T: std::fmt::Display>(
    subject: &str,
    valid: impl IntoIterator<Item = T>,
) -> String {
    let listed = valid
        .into_iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("Invalid {subject} value. Valid values are: [{listed}]")
}

/// Encode a pagination cursor as an opaque token.
///
/// The payload is this service's own structure; hex-encoding it keeps the token
/// opaque to a client, which must only round-trip it back to the next call.
pub fn encode_token(payload: &serde_json::Value) -> String {
    hex::encode(serde_json::to_vec(payload).unwrap_or_default())
}

/// Decode a token [`encode_token`] produced. `None` for anything else, which a
/// handler refuses as an invalid `nextToken`.
pub fn decode_token(token: &str) -> Option<serde_json::Value> {
    let bytes = hex::decode(token).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudwatchlogsLogs20140328State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudwatchlogsLogs20140328State>)
}
