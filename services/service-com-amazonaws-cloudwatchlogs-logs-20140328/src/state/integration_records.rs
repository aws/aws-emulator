// SPDX-License-Identifier: CC0-1.0

use super::*;

impl LookupTableRecord {
    /// A brand-new lookup table holding `table_body`, whose shape `shape` describes. This is
    /// what `CreateLookupTable` stores; the optional members a create may name —
    /// `description`, `kmsKeyId`, `tags` — are left at their "the request named none" value
    /// and set on the returned record by the caller that has them.
    pub fn new(
        name: &str,
        arn: &str,
        table_body: String,
        shape: TableBodyShape,
        created_at_ms: i64,
    ) -> Self {
        Self {
            name: name.to_string(),
            arn: arn.to_string(),
            description: None,
            table_body,
            table_fields: shape.fields,
            records_count: shape.records_count,
            kms_key_id: None,
            created_at_ms,
            last_updated_time_ms: created_at_ms,
            tags: BTreeMap::new(),
        }
    }

    /// Replace everything this table holds with `table_body`, whose shape `shape` describes,
    /// as of `updated_at_ms`. This is what `UpdateLookupTable` stores: "This is a full
    /// replacement operation. All existing content is replaced", so the column names and the
    /// record count are re-derived from the new body rather than merged with the old one's,
    /// and the instant of the write becomes the table's last-updated instant. The members a
    /// content replacement does not touch — the name, the ARN, the creation instant, the
    /// description, the KMS key and the tags — are left as they are, for the caller to set
    /// from the members its request named.
    pub fn replace_content(
        &mut self,
        table_body: String,
        shape: TableBodyShape,
        updated_at_ms: i64,
    ) {
        self.table_body = table_body;
        self.table_fields = shape.fields;
        self.records_count = shape.records_count;
        self.last_updated_time_ms = updated_at_ms;
    }

    /// `LookupTable$sizeBytes` — "The size of the lookup table in bytes": the length of the
    /// CSV content this table holds, in the UTF-8 encoding `CreateLookupTable` requires of
    /// it. DERIVED from the body rather than stored beside it, so it cannot disagree with
    /// what the table holds.
    pub fn size_bytes(&self) -> i64 {
        i64::try_from(self.table_body.len()).unwrap_or(i64::MAX)
    }

    /// This table as the model's `LookupTable` structure, which is what
    /// `DescribeLookupTables` reports, so one record renders the same way through any reader
    /// of it. The body itself is not a member of that shape — only `GetLookupTable` reports
    /// it — so a listing carries the table's metadata and not its megabytes.
    pub fn reported(&self) -> crate::ops::types::LookupTable {
        crate::ops::types::LookupTable {
            lookup_table_arn: Some(self.arn.clone()),
            lookup_table_name: Some(self.name.clone()),
            description: self.description.clone(),
            table_fields: Some(self.table_fields.clone()),
            records_count: Some(self.records_count),
            size_bytes: Some(self.size_bytes()),
            last_updated_time: Some(self.last_updated_time_ms),
            kms_key_id: self.kms_key_id.clone(),
        }
    }
}

/// A scheduled query's ARN, the form `ScheduledQuerySummary$scheduledQueryArn` ("The ARN of
/// the scheduled query") carries: a `scheduled-query` resource of this service, identified by
/// the query's own name, which is what makes the ARN and the name interchangeable as the
/// `ScheduledQueryIdentifier` every reader accepts.
///
/// This is also the form CloudWatch mints for the query it manages for a log alarm
/// (`ScheduledQueryConfiguration$QueryARN`), which is why that ARN resolves here too
/// ([`log_alarm_queries`]).
pub fn scheduled_query_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("scheduled-query:{name}"))
}

/// The state a scheduled query a create named none for starts in: `CreateScheduledQuery$state`
/// documents "The initial state of the scheduled query. Valid values are ENABLED and DISABLED.
/// Default is ENABLED."
pub const SCHEDULED_QUERY_STATE_ENABLED: &str = "ENABLED";

/// The `ScheduleType` of a scheduled query a caller created: the schedule is the caller's,
/// stated by its own `scheduleExpression`, so it is the customer-managed one of the two values
/// the model enumerates (`CUSTOMER_MANAGED` | `AWS_MANAGED`) — nothing in this store creates
/// an AWS-managed schedule, and no create member names the type.
pub const SCHEDULE_TYPE_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// THE EMULATOR'S OWN CEILING on the scheduled queries one account holds, not a quota cited
/// from AWS: each query keeps its configuration in memory for as long as the process runs and
/// nothing expires one, so `CreateScheduledQuery` refuses to create more than this many with
/// the `ServiceQuotaExceededException` it models ("This request exceeds a service quota")
/// rather than growing without bound on request.
pub const MAX_SCHEDULED_QUERIES: usize = 1_000;

/// One scheduled query — the query `CreateScheduledQuery` schedules "at regular intervals" —
/// as it is stored. Every member of `GetScheduledQuery`'s response is reported from this
/// record, so that reader answers from state alone.
#[derive(Debug, Clone)]
pub struct ScheduledQueryRecord {
    /// `name` — "The name of the scheduled query". Also the key the store is keyed by, because
    /// the name "must be unique within your account and region".
    pub name: String,
    /// `scheduledQueryArn` — the ARN [`scheduled_query_arn`] built for this query in the region
    /// the create was signed for.
    pub arn: String,
    /// `description` — "The description of the scheduled query", when a call named one.
    pub description: Option<String>,
    /// `queryLanguage` — "The query language used by the scheduled query" (`CWLI` | `SQL` |
    /// `PPL`). Required of a create, so every stored query carries one.
    pub query_language: String,
    /// `queryString` — "The query string executed by the scheduled query", verbatim as the call
    /// that wrote it sent it.
    pub query_string: String,
    /// `logGroupIdentifiers` — "The log groups queried by the scheduled query", in the order
    /// the call named them. Optional on a create, so a query naming none holds none and reports
    /// no member at all.
    pub log_group_identifiers: Vec<String>,
    /// `scheduleExpression` — "The cron expression that defines when the scheduled query runs".
    /// Required of a create, so every stored query carries one.
    pub schedule_expression: String,
    /// `timezone` — "The timezone used for evaluating the schedule expression", when a call
    /// named one.
    pub timezone: Option<String>,
    /// `startTimeOffset` — "The time offset in seconds that defines the lookback period for the
    /// query", when a call named one.
    pub start_time_offset: Option<i64>,
    /// `endTimeOffset` — "The time offset in seconds that defines the end of the lookback period
    /// for the query", when a call named one.
    pub end_time_offset: Option<i64>,
    /// `destinationConfiguration` — "Configuration for where query results are delivered", as
    /// the call that wrote it stated it.
    pub destination_configuration: Option<crate::ops::types::DestinationConfiguration>,
    /// `state` — "The current state of the scheduled query" (`ENABLED` | `DISABLED`), the one
    /// the create named or [`SCHEDULED_QUERY_STATE_ENABLED`] when it named none.
    pub state: String,
    /// `scheduleType` — "The schedule type of the scheduled query", which for a query a caller
    /// created is [`SCHEDULE_TYPE_CUSTOMER_MANAGED`].
    pub schedule_type: String,
    /// `scheduleStartTime` — "The start time for the scheduled query in Unix epoch format",
    /// when a call named one.
    pub schedule_start_time: Option<i64>,
    /// `scheduleEndTime` — "The end time for the scheduled query in Unix epoch format", when a
    /// call named one.
    pub schedule_end_time: Option<i64>,
    /// `executionRoleArn` — "The ARN of the IAM role used to execute the query and deliver
    /// results". Required of a create, so every stored query carries one.
    pub execution_role_arn: String,
    /// `creationTime` — "The timestamp when the scheduled query was created", in epoch
    /// milliseconds. `None` for a query whose owner records no instant of its creation, which
    /// is a query the SERVICE manages rather than one a create minted
    /// ([`super::state::log_alarm_queries`]).
    pub creation_time_ms: Option<i64>,
    /// `lastUpdatedTime` — "The timestamp when the scheduled query was last updated", in epoch
    /// milliseconds. A query nothing has updated since it was created reports the instant it
    /// was created.
    pub last_updated_time_ms: Option<i64>,
    /// `lastTriggeredTime` — "The timestamp when the scheduled query was last executed", in
    /// epoch milliseconds, and `lastExecutionStatus` — "The status of the most recent execution
    /// of the scheduled query". `None` on a query no execution has been recorded for, which is
    /// every query until its own schedule makes its first run due
    /// ([`super::scheduled_query_executions`]): both members report an execution that happened,
    /// so neither is reported for one that has not.
    pub last_triggered_time_ms: Option<i64>,
    /// See [`Self::last_triggered_time_ms`].
    pub last_execution_status: Option<String>,
    /// Every execution recorded for this query — the runs its own schedule has made due, which
    /// `GetScheduledQueryHistory` reports — oldest first, bounded by
    /// [`super::MAX_TRIGGER_HISTORY_RECORDS`]. Empty until the first run falls due.
    pub trigger_history: Vec<super::ScheduledQueryExecution>,
    /// The tags `CreateScheduledQuery$tags` assigned to this query. A `BTreeMap` so the stored
    /// order is stable; no response member of a scheduled query reports them, so they are read
    /// back through the tagging operations once those reach this resource.
    pub tags: BTreeMap<String, String>,
}

impl ScheduledQueryRecord {
    /// A scheduled query a caller created, from the members a create must name.
    ///
    /// Everything else — the description, the log groups, the timezone, the offsets, the
    /// destination configuration, the schedule window, the state, the tags and the instants —
    /// is left at its "the request named none" value, which for the state is the documented
    /// default ([`SCHEDULED_QUERY_STATE_ENABLED`]), and set on the returned record by the
    /// caller that has them.
    pub fn new(
        name: &str,
        arn: &str,
        query_language: &str,
        query_string: &str,
        schedule_expression: &str,
        execution_role_arn: &str,
    ) -> Self {
        Self {
            name: name.to_string(),
            arn: arn.to_string(),
            description: None,
            query_language: query_language.to_string(),
            query_string: query_string.to_string(),
            log_group_identifiers: Vec::new(),
            schedule_expression: schedule_expression.to_string(),
            timezone: None,
            start_time_offset: None,
            end_time_offset: None,
            destination_configuration: None,
            state: SCHEDULED_QUERY_STATE_ENABLED.to_string(),
            schedule_type: SCHEDULE_TYPE_CUSTOMER_MANAGED.to_string(),
            schedule_start_time: None,
            schedule_end_time: None,
            execution_role_arn: execution_role_arn.to_string(),
            creation_time_ms: None,
            last_updated_time_ms: None,
            last_triggered_time_ms: None,
            last_execution_status: None,
            // No run has been recorded for a query that was just written.
            trigger_history: Vec::new(),
            tags: BTreeMap::new(),
        }
    }

    /// The scheduled query the SERVICE manages for a log alarm, from the members that alarm's
    /// `ScheduledQueryConfiguration` carries: its schedule type is
    /// [`log_alarm_queries::SCHEDULE_TYPE_AWS_MANAGED`] rather than the caller's, and its query
    /// is in this service's default query language ([`DEFAULT_QUERY_LANGUAGE`]), which the
    /// alarm states no language for.
    ///
    /// `arn` is the ARN the alarm's configuration names the query by
    /// (`ScheduledQueryConfiguration$QueryARN`). The rest is set by the caller, as for
    /// [`Self::new`].
    pub fn managed(
        name: &str,
        arn: String,
        query_string: &str,
        schedule_expression: &str,
        execution_role_arn: &str,
    ) -> Self {
        Self {
            schedule_type: log_alarm_queries::SCHEDULE_TYPE_AWS_MANAGED.to_string(),
            ..Self::new(
                name,
                &arn,
                DEFAULT_QUERY_LANGUAGE,
                query_string,
                schedule_expression,
                execution_role_arn,
            )
        }
    }
}

/// One integration between CloudWatch Logs and another service — the resource
/// `PutIntegration` creates, `GetIntegration` and `ListIntegrations` report and
/// `DeleteIntegration` removes — as it is stored.
///
/// `PutIntegration` is the only call that creates one and "You can't modify an existing
/// integration", so every member here is set once, at create time, from the request's
/// `resourceConfig`; the only thing that moves afterwards is [`Self::status`], which the
/// CLOCK owes ([`Self::status_due_ms`]).
#[derive(Debug, Clone)]
pub struct IntegrationRecord {
    /// `IntegrationSummary$integrationName` — "A name for the integration", which is also
    /// the key the store is keyed by: `GetIntegration` and `DeleteIntegration` identify an
    /// integration by nothing else.
    pub name: String,
    /// `IntegrationSummary$integrationType`, one value of the model's `IntegrationType`
    /// enum ([`INTEGRATION_TYPE_OPENSEARCH`]).
    pub integration_type: String,
    /// `IntegrationSummary$integrationStatus`, one value of the model's `IntegrationStatus`
    /// enum ([`INTEGRATION_STATUS_PROVISIONING`] and its family).
    pub status: String,
    /// The region the create was signed in, so the ARNs this integration's OpenSearch
    /// Service resources are reported under do not move when a later read arrives in
    /// another credential scope.
    pub region: String,
    /// `OpenSearchResourceConfig$kmsKeyArn` — the key "the vended dashboard data" is
    /// encrypted with, when the create named one; `None` leaves "the CloudWatch Logs
    /// default encryption method".
    pub kms_key_arn: Option<String>,
    /// `OpenSearchResourceConfig$dataSourceRoleArn` — "the ARN of an IAM role that
    /// CloudWatch Logs will use to create the integration".
    pub data_source_role_arn: String,
    /// `OpenSearchResourceConfig$dashboardViewerPrincipals` — "the ARNs of IAM roles and
    /// IAM users who you want to grant permission to for viewing the dashboards".
    pub dashboard_viewer_principals: Vec<String>,
    /// The existing OpenSearch Service application the create named, if it named one:
    /// "If you want to use an existing OpenSearch Service application for your integration
    /// [...] specify it here. If you omit this, a new application will be created" — which
    /// is why this is `None` for an integration whose application the setup created, and
    /// [`Self::application_arn`] is what a reader reports either way.
    pub existing_application_arn: Option<String>,
    /// `OpenSearchResourceConfig$retentionDays` — "how many days that you want the data
    /// derived by OpenSearch Service to be retained in the index that the dashboard refers
    /// to", exactly as the create named it.
    pub retention_days: i32,
    /// When this integration was created, in epoch milliseconds. The instant the
    /// provisioning transition is measured from.
    pub created_at_ms: i64,
    /// The position this integration takes in the account's creation order, from the
    /// store's monotonic integration counter: what `ListIntegrations` enumerates by, so
    /// the order it reports does not move when another integration is created or deleted.
    pub creation_seq: u64,
    /// The instant this integration is due to reach its next status, stored rather than
    /// waited for: a reader materialises the transition once the clock is past it
    /// ([`IntegrationRecord::materialize_due_transitions`]). `None` once it is in a status
    /// the clock owes nothing after.
    pub status_due_ms: Option<i64>,
}

impl IntegrationRecord {
    /// A brand-new integration over the configuration the create named: still
    /// provisioning, and due to become [`INTEGRATION_STATUS_ACTIVE`]
    /// [`INTEGRATION_PROVISIONING_DELAY_MS`] after it was created. This is what
    /// `PutIntegration` stores.
    pub fn new(
        region: &str,
        name: &str,
        integration_type: &str,
        config: &crate::ops::types::OpenSearchResourceConfig,
        created_at_ms: i64,
        creation_seq: u64,
    ) -> Self {
        Self {
            name: name.to_string(),
            integration_type: integration_type.to_string(),
            status: INTEGRATION_STATUS_PROVISIONING.to_string(),
            region: region.to_string(),
            kms_key_arn: config.kms_key_arn.clone(),
            data_source_role_arn: config.data_source_role_arn.clone(),
            dashboard_viewer_principals: config.dashboard_viewer_principals.clone(),
            existing_application_arn: config.application_arn.clone(),
            retention_days: config.retention_days,
            created_at_ms,
            creation_seq,
            status_due_ms: Some(created_at_ms.saturating_add(INTEGRATION_PROVISIONING_DELAY_MS)),
        }
    }

    /// Materialise the status change the clock owes this integration: one whose next
    /// status was due before `now_ms` is already in it — the change taken AT the instant it
    /// was due, not at the time something happened to look.
    pub fn materialize_due_transitions(&mut self, now_ms: i64) {
        while let Some(due) = self.status_due_ms {
            if due > now_ms {
                return;
            }
            match self.status.as_str() {
                // "After you create an integration, it takes a few minutes to complete.
                // During this time, you'll see the status as PROVISIONING" — so an
                // integration leaves that status on the clock, with nothing further owed
                // after it.
                INTEGRATION_STATUS_PROVISIONING => {
                    self.status = INTEGRATION_STATUS_ACTIVE.to_string();
                    self.status_due_ms = None;
                }
                // Every other status is one the clock owes nothing after.
                _ => self.status_due_ms = None,
            }
        }
    }

    /// This integration as the model's `IntegrationSummary`, which is what
    /// `ListIntegrations` reports, so one record renders the same way through any reader.
    pub fn summary(&self) -> crate::ops::types::IntegrationSummary {
        crate::ops::types::IntegrationSummary {
            integration_name: Some(self.name.clone()),
            integration_type: Some(self.integration_type.clone()),
            integration_status: Some(self.status.clone()),
        }
    }

    /// `OpenSearchDataSource$dataSourceName` — the data source "created as part of the
    /// integration setup", named after the integration it belongs to.
    pub fn data_source_name(&self) -> String {
        self.name.clone()
    }

    /// `OpenSearchApplication$applicationArn` — the existing application the create named,
    /// or the ARN of the one the setup created for it.
    pub fn application_arn(&self) -> String {
        self.existing_application_arn.clone().unwrap_or_else(|| {
            ::engine::emulator_arn(
                OPENSEARCH_SERVICE,
                &self.region,
                &format!("application/{}", self.application_id()),
            )
        })
    }

    /// `OpenSearchApplication$applicationId` — the id the application's ARN carries, which
    /// is the last segment of its resource part for an application the create named, and
    /// the derived id otherwise.
    pub fn application_id(&self) -> String {
        match self
            .existing_application_arn
            .as_deref()
            .and_then(arn_service_and_resource)
            .and_then(|(_, resource)| resource.split('/').next_back())
        {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => format!("{}{}", self.name, APPLICATION_SUFFIX),
        }
    }

    /// `OpenSearchCollection$collectionArn` — the serverless collection the setup created
    /// for this integration, which the dashboards query.
    pub fn collection_arn(&self) -> String {
        ::engine::emulator_arn(
            OPENSEARCH_SERVERLESS_SERVICE,
            &self.region,
            &format!("collection/{}{}", self.name, COLLECTION_SUFFIX),
        )
    }

    /// `OpenSearchWorkspace$workspaceId` — the workspace the setup created for this
    /// integration.
    pub fn workspace_id(&self) -> String {
        format!("{}{}", self.name, WORKSPACE_SUFFIX)
    }

    /// The status every OpenSearch Service resource of this integration is in, derived from
    /// the integration's own status: the setup creates them all together, so until the
    /// integration has finished provisioning none of them is there
    /// ([`OPENSEARCH_RESOURCE_STATUS_NOT_FOUND`]), once it is active they all are, and an
    /// integration that failed reports them in error.
    pub(super) fn resource_status(&self) -> crate::ops::types::OpenSearchResourceStatus {
        let status = match self.status.as_str() {
            INTEGRATION_STATUS_ACTIVE => OPENSEARCH_RESOURCE_STATUS_ACTIVE,
            INTEGRATION_STATUS_PROVISIONING => OPENSEARCH_RESOURCE_STATUS_NOT_FOUND,
            _ => OPENSEARCH_RESOURCE_STATUS_ERROR,
        };
        crate::ops::types::OpenSearchResourceStatus {
            status: Some(status.to_string()),
            status_message: None,
        }
    }

    /// This integration as the model's `IntegrationDetails` union — "information about
    /// OpenSearch Service resources such as the collection, the workspace, and policies" —
    /// which is what `GetIntegration` reports.
    ///
    /// Every name, id and ARN here is derived from the integration itself, so two reads of
    /// one integration report the same resources. No endpoint is reported: this emulator
    /// stands up no OpenSearch Service collection or application, so there is no host a
    /// client could reach, and `applicationEndpoint` / `collectionEndpoint` are left absent
    /// rather than filled with one nothing answers on.
    pub fn details(&self) -> crate::ops::types::IntegrationDetails {
        let status = self.resource_status();
        crate::ops::types::IntegrationDetails::openSearchIntegrationDetails(
            crate::ops::types::OpenSearchIntegrationDetails {
                data_source: Some(crate::ops::types::OpenSearchDataSource {
                    data_source_name: Some(self.data_source_name()),
                    status: Some(status.clone()),
                }),
                application: Some(crate::ops::types::OpenSearchApplication {
                    application_endpoint: None,
                    application_arn: Some(self.application_arn()),
                    application_id: Some(self.application_id()),
                    status: Some(status.clone()),
                }),
                collection: Some(crate::ops::types::OpenSearchCollection {
                    collection_endpoint: None,
                    collection_arn: Some(self.collection_arn()),
                    status: Some(status.clone()),
                }),
                workspace: Some(crate::ops::types::OpenSearchWorkspace {
                    workspace_id: Some(self.workspace_id()),
                    status: Some(status.clone()),
                }),
                encryption_policy: Some(crate::ops::types::OpenSearchEncryptionPolicy {
                    policy_name: Some(format!("{}{}", self.name, ENCRYPTION_POLICY_SUFFIX)),
                    status: Some(status.clone()),
                }),
                network_policy: Some(crate::ops::types::OpenSearchNetworkPolicy {
                    policy_name: Some(format!("{}{}", self.name, NETWORK_POLICY_SUFFIX)),
                    status: Some(status.clone()),
                }),
                access_policy: Some(crate::ops::types::OpenSearchDataAccessPolicy {
                    policy_name: Some(format!("{}{}", self.name, ACCESS_POLICY_SUFFIX)),
                    status: Some(status.clone()),
                }),
                lifecycle_policy: Some(crate::ops::types::OpenSearchLifecyclePolicy {
                    policy_name: Some(format!("{}{}", self.name, LIFECYCLE_POLICY_SUFFIX)),
                    status: Some(status),
                }),
            },
        )
    }
}

/// One data-source association of an S3 Table Integration as it is stored: the model's
/// `S3TableIntegrationSource` plus the integration it belongs to (the `integrationArn` the
/// associate named, which only `ListSourcesForS3TableIntegration` reads back, as the
/// integration it lists the sources of) and the instant its deletion is due.
///
/// The integration itself is NOT a resource this service holds: `PutIntegration`'s
/// `integrationType` admits only `OPENSEARCH`, so no operation here creates an S3 Table
/// Integration and the ARN the associate names is the only thing identifying one. An
/// association is therefore stored against that ARN as the request gave it, and the
/// family's reader lists the associations whose ARN matches.
#[derive(Debug, Clone)]
pub struct S3TableIntegrationSourceRecord {
    /// `S3TableIntegrationSource$identifier` — "The unique identifier for this data source
    /// association", minted by the associate
    /// ([`mint_s3_table_integration_source_identifier`]) and the key this record is stored
    /// under.
    pub identifier: String,
    /// The `integrationArn` the associate named: "The Amazon Resource Name (ARN) of the S3
    /// Table Integration to associate the data source with", which is what
    /// `ListSourcesForS3TableIntegration` selects an integration's associations by.
    pub integration_arn: String,
    /// `S3TableIntegrationSource$dataSource` — "The data source associated with the S3
    /// Table Integration", exactly as the associate named it. Held as the
    /// [`DataSourceRecord`] every data source of this service is held as; an association's
    /// carries no format, because the request's `DataSource` has no such member.
    pub data_source: DataSourceRecord,
    /// `S3TableIntegrationSource$status` — "The current status of the data source
    /// association", one of the model's `S3TableIntegrationSourceStatus` values.
    pub status: String,
    /// `S3TableIntegrationSource$createdTimeStamp` — "The timestamp when the data source
    /// association was created", in epoch milliseconds.
    pub created_time_stamp_ms: i64,
    /// `S3TableIntegrationSource$parentSourceIdentifier` — "The identifier of the parent
    /// data source for this association". `None` for an association a request made: the
    /// associate names one data source and no parent, so nothing it creates has one.
    pub parent_source_identifier: Option<String>,
    /// The instant this association is due to be GONE, stored rather than waited for: a
    /// reader drops it once the clock is past it
    /// ([`ComAmazonawsCloudwatchlogsLogs20140328State::materialize_due_s3_table_integration_source_removals`]).
    /// `None` while the association is still associated, which is also what tells an
    /// associate of the same pair that it names this association rather than a new one.
    pub removal_due_ms: Option<i64>,
}

impl S3TableIntegrationSourceRecord {
    /// A brand-new association of `data_source` with the integration `integration_arn`,
    /// giving query access as of `created_time_stamp_ms`
    /// ([`S3_TABLE_INTEGRATION_SOURCE_STATUS_ACTIVE`]). This is what
    /// `AssociateSourceToS3TableIntegration` stores.
    pub fn new(
        identifier: &str,
        integration_arn: &str,
        data_source: &crate::ops::types::DataSource,
        created_time_stamp_ms: i64,
    ) -> Self {
        Self {
            identifier: identifier.to_string(),
            integration_arn: integration_arn.to_string(),
            data_source: DataSourceRecord {
                name: data_source.name.clone(),
                source_type: data_source.r#type.clone(),
                format: None,
            },
            status: S3_TABLE_INTEGRATION_SOURCE_STATUS_ACTIVE.to_string(),
            created_time_stamp_ms,
            parent_source_identifier: None,
            removal_due_ms: None,
        }
    }

    /// Take this association's query access away and leave its data deletion running, as of
    /// `now_ms`: what `DisassociateSourceFromS3TableIntegration` does. An association whose
    /// deletion is already running keeps the instant it was first due, so a second
    /// disassociate does not postpone it.
    pub fn start_removal(&mut self, now_ms: i64) {
        if self.removal_due_ms.is_some() {
            return;
        }
        self.status = S3_TABLE_INTEGRATION_SOURCE_STATUS_DELETE_IN_PROGRESS.to_string();
        self.removal_due_ms =
            Some(now_ms.saturating_add(S3_TABLE_INTEGRATION_SOURCE_DELETION_DELAY_MS));
    }

    /// Whether the clock has already finished deleting this association's data, so the
    /// association itself is gone as of `now_ms`.
    pub fn removal_due(&self, now_ms: i64) -> bool {
        self.removal_due_ms.is_some_and(|due| due <= now_ms)
    }

    /// This association as the model's `S3TableIntegrationSource`, which is what
    /// `ListSourcesForS3TableIntegration` reports, so one record renders the same way
    /// through any reader.
    ///
    /// No `statusReason` is reported: the member is "additional information about the
    /// status", and both statuses an association of this emulator reaches are already said
    /// by the status itself.
    pub fn source(&self) -> crate::ops::types::S3TableIntegrationSource {
        crate::ops::types::S3TableIntegrationSource {
            identifier: Some(self.identifier.clone()),
            data_source: Some(crate::ops::types::DataSource {
                name: self.data_source.name.clone(),
                r#type: self.data_source.source_type.clone(),
            }),
            status: Some(self.status.clone()),
            status_reason: None,
            created_time_stamp: Some(self.created_time_stamp_ms),
            parent_source_identifier: self.parent_source_identifier.clone(),
        }
    }
}
