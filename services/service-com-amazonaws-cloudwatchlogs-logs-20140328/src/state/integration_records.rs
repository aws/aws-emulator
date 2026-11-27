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
    /// when a call named one, in the epoch milliseconds this service states an epoch instant
    /// in ([`super::scheduled_query_executions`]).
    pub schedule_start_time: Option<i64>,
    /// `scheduleEndTime` — "The end time for the scheduled query in Unix epoch format", when a
    /// call named one, in the same unit.
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
    /// The `host[:port]` this emulator answers on, as the create that minted this integration
    /// resolved it ([`emulator_authority`]), and `None` for an emulator that is served on no
    /// address of its own.
    ///
    /// The two endpoints a read reports are addresses a CLIENT DIALS, so they have to lead
    /// back to this process ([`opensearch_resource_endpoint`]). The address is resolved ONCE,
    /// by the create, and stored: an endpoint a client keeps does not move between two reads
    /// of one integration.
    pub announced_authority: Option<String>,
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
        announced_authority: Option<String>,
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
            announced_authority,
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
            &self.collection_resource(),
        )
    }

    /// The RESOURCE part of this integration's collection ARN: `collection/{name}`, the form
    /// OpenSearch Serverless names a collection by, over the collection's own name
    /// ([`Self::collection_name`]).
    fn collection_resource(&self) -> String {
        format!("collection/{}", self.collection_name())
    }

    /// The name of the serverless collection this integration's setup created, after the
    /// integration it belongs to. The ONE derivation of it, read by the ARN and by the
    /// endpoint, so neither can name a second collection.
    fn collection_name(&self) -> String {
        format!("{}{}", self.name, COLLECTION_SUFFIX)
    }

    /// `OpenSearchCollection$collectionEndpoint` — "The endpoint of the collection", which is
    /// where a client queries the indexes the integration's dashboards read.
    ///
    /// The collection the setup created lives in the Region the integration was created in, so
    /// its endpoint is its own name in that Region's OpenSearch Serverless endpoint zone
    /// ([`opensearch_resource_endpoint`]).
    pub fn collection_endpoint(&self) -> String {
        opensearch_resource_endpoint(
            COLLECTION_RESOURCE,
            &self.published_resource_host(COLLECTION_RESOURCE),
            self.announced_authority.as_deref(),
        )
    }

    /// `OpenSearchApplication$applicationEndpoint` — "The endpoint of the application": the
    /// address a viewer opens the vended logs dashboards this integration's application hosts
    /// at.
    ///
    /// The application is the one the create named or the one the setup created for it
    /// ([`Self::application_id`]), so the endpoint is that application's own id in the Region
    /// the application lives in — the Region an existing application's ARN names, and the
    /// integration's own for one the setup created ([`Self::application_region`]).
    pub fn application_endpoint(&self) -> String {
        opensearch_resource_endpoint(
            APPLICATION_RESOURCE,
            &self.published_resource_host(APPLICATION_RESOURCE),
            self.announced_authority.as_deref(),
        )
    }

    /// The host AWS itself publishes the resource of kind `kind` of this integration at: the
    /// resource's own name in its service's regional endpoint zone
    /// ([`aws_resource_endpoint_host`]).
    ///
    /// It is what the announced endpoint carries and what the plane serving that endpoint reads
    /// a request's resource back from ([`integration_resource_path`]), so one derivation names
    /// the resource on both sides. A kind this family publishes no endpoint for names the
    /// APPLICATION's host, because the application is the resource an integration is addressed
    /// through — but no caller reaches that branch: the only kinds addressed are the two a read
    /// reports ([`addressed_integration_resource`]).
    pub fn published_resource_host(&self, kind: &str) -> String {
        if kind == COLLECTION_RESOURCE {
            return aws_resource_endpoint_host(
                &self.collection_name(),
                &self.region,
                OPENSEARCH_SERVERLESS_SERVICE,
            );
        }
        aws_resource_endpoint_host(
            &self.application_id(),
            self.application_region(),
            OPENSEARCH_SERVICE,
        )
    }

    /// The Region this integration's OpenSearch Service application lives in: the one the ARN
    /// of an existing application names, and the integration's own Region for an application
    /// the setup created there.
    fn application_region(&self) -> &str {
        self.existing_application_arn
            .as_deref()
            .and_then(arn_region)
            .unwrap_or(&self.region)
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
    ///
    /// The status MESSAGE is "additional information about the status of this resource", and
    /// a resource whose status says everything there is to say about it has none to add — so
    /// the member is reported EMPTY rather than omitted, which is what the service answers
    /// for every resource of an integration it set up.
    pub fn resource_status(&self) -> crate::ops::types::OpenSearchResourceStatus {
        let status = match self.status.as_str() {
            INTEGRATION_STATUS_ACTIVE => OPENSEARCH_RESOURCE_STATUS_ACTIVE,
            INTEGRATION_STATUS_PROVISIONING => OPENSEARCH_RESOURCE_STATUS_NOT_FOUND,
            _ => OPENSEARCH_RESOURCE_STATUS_ERROR,
        };
        crate::ops::types::OpenSearchResourceStatus {
            status: Some(status.to_string()),
            status_message: Some(NO_STATUS_MESSAGE.to_string()),
        }
    }

    /// This integration as the model's `IntegrationDetails` union — "information about
    /// OpenSearch Service resources such as the collection, the workspace, and policies" —
    /// which is what `GetIntegration` reports.
    ///
    /// Every name, id, ARN and ENDPOINT here is derived from the integration itself, so two
    /// reads of one integration report the same resources. The endpoints of the two resources
    /// a client addresses — the application that hosts the dashboards and the collection they
    /// query — are reported beside those resources' ARNs and ids, because an endpoint is part
    /// of the identity the setup assigns a resource; whether the resource is there YET is what
    /// its `status` says ([`Self::resource_status`]), and that status is also where the status
    /// MESSAGE is reported.
    pub fn details(&self) -> crate::ops::types::IntegrationDetails {
        let status = self.resource_status();
        crate::ops::types::IntegrationDetails::openSearchIntegrationDetails(
            crate::ops::types::OpenSearchIntegrationDetails {
                data_source: Some(crate::ops::types::OpenSearchDataSource {
                    data_source_name: Some(self.data_source_name()),
                    status: Some(status.clone()),
                }),
                application: Some(crate::ops::types::OpenSearchApplication {
                    application_endpoint: Some(self.application_endpoint()),
                    application_arn: Some(self.application_arn()),
                    application_id: Some(self.application_id()),
                    status: Some(status.clone()),
                }),
                collection: Some(crate::ops::types::OpenSearchCollection {
                    collection_endpoint: Some(self.collection_endpoint()),
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

/// Where a client reaches the OpenSearch Service resource of kind `kind` that an integration's
/// setup created, published at `published_host` ([`aws_resource_endpoint_host`]).
///
/// An endpoint is not a label a caller reads back: it is an address the caller then DIALS —
/// "the documented next step after the integration is created" — so it has to lead back to
/// THIS process. The address is therefore the `host[:port]` this emulator answers on
/// (`announced_authority`, resolved by the create through [`emulator_authority`]) followed by
/// the path this service SERVES that resource at ([`integration_resource_path`]), which is
/// where a client on the machine this emulator runs on can carry what it cannot carry in the
/// host.
///
/// THE FORM is the one both endpoint members' `@pattern` admits — `^https://[\.\-_/#:A-Za-z0-9]+\.com$`
/// (`smithy_query(query="shape",shape="OpenSearchCollectionEndpoint")`), which is also what
/// `OpenSearchApplicationEndpoint` carries: the expression ends the whole value in the endpoint
/// zone AWS publishes its own endpoints in ([`aws_endpoint_zone`]), so the resource's published
/// host stands at the END of the value — as the path segment naming the resource when there is
/// an address of this emulator's to put in front of it, and as the host itself for an emulator
/// with no address the member can carry ([`endpoint_member_admits`]), which has none to
/// announce.
fn opensearch_resource_endpoint(
    kind: &str,
    published_host: &str,
    announced_authority: Option<&str>,
) -> String {
    match announced_authority.filter(|authority| endpoint_member_admits(authority)) {
        Some(authority) => format!(
            "https://{authority}{}",
            integration_resource_path(kind, published_host)
        ),
        None => format!("https://{published_host}"),
    }
}

/// The path segment naming THIS service's integration-resource plane, under the reserved
/// namespace every plane this emulator serves on its own endpoint sits beneath
/// (`::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`): this service's own
/// name and the service whose resources it addresses, so neither OpenSearch Service's own
/// application plane nor OpenSearch Serverless's own collection plane can claim a path of this
/// one, however a resource is named.
pub const INTEGRATION_PLANE_SEGMENT: &str = "logs-opensearch";

/// The two resources of an integration a client ADDRESSES, spelled as the members a read
/// reports them under (`OpenSearchIntegrationDetails$application` and `$collection`).
pub const APPLICATION_RESOURCE: &str = "application";
/// See [`APPLICATION_RESOURCE`].
pub const COLLECTION_RESOURCE: &str = "collection";

/// The path the resource of kind `kind` published at `published_host` is SERVED at on this
/// emulator's own endpoint: `/_aws/logs-opensearch/<kind>/<published host>`.
///
/// Shared by the endpoint a read announces ([`opensearch_resource_endpoint`]) and by the plane
/// that answers it ([`crate::service::integration_endpoints`]), so the address this service
/// publishes is the address it serves. The published host is ONE path segment: it is a host
/// name, which carries no `/`.
pub fn integration_resource_path(kind: &str, published_host: &str) -> String {
    format!(
        "/{}/{INTEGRATION_PLANE_SEGMENT}/{kind}/{published_host}",
        ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE
    )
}

/// What an announced path ADDRESSES: the kind of resource, the published host naming it, and
/// the route beneath that resource's endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressedIntegrationResource {
    /// [`APPLICATION_RESOURCE`] or [`COLLECTION_RESOURCE`].
    pub kind: String,
    /// The host the resource is published at, which is the segment that names it.
    pub published_host: String,
    /// The route the request asked for beneath the endpoint; `/` for the endpoint itself.
    pub route: String,
}

/// The resource `path` addresses, and `None` for a path that is no endpoint this service
/// announces.
///
/// The inverse of [`integration_resource_path`], read through the shared addressing module, so
/// a URL this service published resolves back to the resource behind it. Only the two kinds a
/// read reports an endpoint for are addressed here; a path naming anything else is this
/// plane's for no resource at all.
pub fn addressed_integration_resource(path: &str) -> Option<AddressedIntegrationResource> {
    use ::service_common::data_plane_addressing::{first_segment, EMULATOR_PATH_NAMESPACE};
    let (namespace, rest) = first_segment(path)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, rest) = first_segment(rest)?;
    if plane != INTEGRATION_PLANE_SEGMENT {
        return None;
    }
    let (kind, rest) = first_segment(rest)?;
    if kind != APPLICATION_RESOURCE && kind != COLLECTION_RESOURCE {
        return None;
    }
    let (published_host, route) = first_segment(rest)?;
    (!published_host.is_empty()).then(|| AddressedIntegrationResource {
        kind: kind.to_string(),
        published_host: published_host.to_string(),
        route: route.to_string(),
    })
}

/// Whether `authority` is a value these endpoint members can CARRY: every character of it is one
/// their `@pattern` admits (`[\.\-_/#:A-Za-z0-9]`).
///
/// A bracketed IPv6 authority is not — `[` and `]` are outside that class — so an emulator
/// reached at one has no address it can announce HERE, and the resource's published host stands
/// instead ([`opensearch_resource_endpoint`]), exactly as it does for an emulator served on no
/// address at all. A value outside the member's own constraint is not an address a client could
/// use either: a validating SDK rejects it before it ever dials.
fn endpoint_member_admits(authority: &str) -> bool {
    !authority.is_empty()
        && authority
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-_/#:".contains(character))
}

/// The host AWS itself publishes the resource `resource` of `service` at in `region`: the
/// resource's own name as the first label of its service's regional endpoint host. It names
/// the resource and the Region the resource lives in, and it is what the announced endpoint
/// carries ([`opensearch_resource_endpoint`]).
///
/// The name is folded to ONE DNS label through the shared reading of that
/// ([`::service_common::data_plane_addressing::dns_label`]), because an integration's name may
/// carry characters a host label does not (`IntegrationName`'s pattern admits `/` and `#`) and
/// a resource name is what distinguishes one integration's endpoints from another's.
fn aws_resource_endpoint_host(resource: &str, region: &str, service: &str) -> String {
    format!(
        "{}.{region}.{service}.{}",
        ::service_common::data_plane_addressing::dns_label(resource),
        aws_endpoint_zone()
    )
}

/// The address this emulator answers on for the client that made this request: the emulator's
/// own endpoint the request reached, else the address this process is served on, and `None` for
/// a request that addressed one of AWS's own hosts on an emulator served on no address at all
/// — which has none of its own to announce.
///
/// The tree's one reading of that question
/// (`::service_common::data_plane_addressing::reached_emulator_endpoint` /
/// `served_endpoint_authority`), so what this service announces is the address every other
/// service announces.
pub fn emulator_authority(
    parts: &::http::request::Parts,
    served_base: &str,
) -> Option<String> {
    ::service_common::data_plane_addressing::reached_emulator_endpoint_of_parts(parts).or_else(
        || ::service_common::data_plane_addressing::served_endpoint_authority(served_base),
    )
}

/// The DNS zone AWS publishes its own endpoints in, without the leading dot the shared list of
/// those zones carries: its first entry, the commercial partition's `dnsSuffix` — the
/// partition every ARN this emulator mints names. Read off that list rather than spelled again
/// here, so this service states one zone with every other reader of it.
fn aws_endpoint_zone() -> &'static str {
    ::service_common::data_plane_addressing::AWS_ENDPOINT_DNS_SUFFIXES[0].trim_start_matches('.')
}

/// The `OpenSearchResourceStatus$statusMessage` of a resource whose status needs nothing
/// added to it: the member is "a message with additional information about the status", and
/// there is none for a resource the setup simply created, so the message is EMPTY. It is
/// still reported rather than omitted, because a client reads the member on every resource.
pub const NO_STATUS_MESSAGE: &str = "";

/// The characters an ARN takes at least, which every `Arn`-typed member of the
/// S3-Table-Integration family is held to before anything tries to resolve a resource from
/// it. That family's requests are judged by the modelled request validator, so the refusal
/// is the standard constraint clause naming the member and this bound
/// ([`short_arn_message`]) rather than a complaint about parsing.
pub const MIN_ARN_CHARS: usize = 20;

/// The characters an ARN may be long at most, the second half of the length bound an
/// `Arn`-typed member is held to. RECORDED behavior, like the minimum beside it: the Smithy
/// model carries no constraint trait on `Arn`, and the constraint set this service's refusals
/// state for one names this maximum ([`arn_constraint_set`]).
pub const MAX_ARN_CHARS: usize = 2048;

/// The expression an `Arn`-typed member must satisfy, as the SERVICE publishes it in a
/// refusal: the characters an ARN is written in, with the trailing `*` a wildcard ARN ends
/// with.
///
/// Stated UNANCHORED, which is the form this service's pattern-rendering rule states for a
/// published expression (see `requirement` in `crate::service`), and the ONE definition the
/// refusal's text and the check itself both read — so the constraint a request is held to
/// cannot drift from the constraint the refusal states. RECORDED behavior: the model declares
/// no `@pattern` on `Arn`.
pub const ARN_PATTERN: &str = r"[\w#+=/:,.@-]*\*?";

/// [`ARN_PATTERN`] compiled once, ANCHORED: a `@pattern` is a requirement of the whole value,
/// so what the published expression admits is a value it matches end to end.
static ARN_EXPRESSION: std::sync::LazyLock<::regex::Regex> = std::sync::LazyLock::new(|| {
    ::regex::Regex::new(&format!("^(?:{ARN_PATTERN})$"))
        .expect("the published Arn expression compiles")
});

/// The whole constraint set this service holds an `Arn`-typed member to, as the requirement
/// sentences a clause states them in: the maximum length, the minimum length and the
/// expression, in that order.
///
/// THE ORDER is AWS's own for this member's set, and the sentences are the shared ones every
/// constraint of that class is worded with
/// ([`::service_common::validation_message::requirement`]), so a clause here cannot word a
/// bound differently from one the validating walk raised. The SET is what a clause at a
/// COLLECTION of ARNs states ([`arn_element_set`]) — a collection's clause states what its
/// elements have to satisfy, not what one element happened to break — while a clause at a
/// single ARN states the one constraint that value failed ([`arn_requirement`]).
pub fn arn_constraint_set() -> Vec<String> {
    use ::aws_smithy_schema_server::constraints::ViolationKind;
    use ::service_common::validation_message::requirement;
    vec![
        requirement(ViolationKind::MaxLength(as_bound(MAX_ARN_CHARS))),
        requirement(ViolationKind::MinLength(as_bound(MIN_ARN_CHARS))),
        requirement(ViolationKind::Pattern(ARN_PATTERN.to_string())),
    ]
}

/// A character count as a `@length` bound is stated, which is how the sentence for it is
/// rendered.
fn as_bound(characters: usize) -> i64 {
    i64::try_from(characters).unwrap_or(i64::MAX)
}

/// The requirement the value of an `Arn`-typed member FAILED — the first one of
/// [`arn_constraint_set`] it breaks, which is the single requirement a clause at that member
/// states — or `None` for a value inside every constraint of the set.
pub fn arn_requirement(value: &str) -> Option<String> {
    use ::aws_smithy_schema_server::constraints::ViolationKind;
    use ::service_common::validation_message::requirement;
    let characters = value.chars().count();
    if characters > MAX_ARN_CHARS {
        return Some(requirement(ViolationKind::MaxLength(as_bound(
            MAX_ARN_CHARS,
        ))));
    }
    if characters < MIN_ARN_CHARS {
        return Some(requirement(ViolationKind::MinLength(as_bound(
            MIN_ARN_CHARS,
        ))));
    }
    (!ARN_EXPRESSION.is_match(value))
        .then(|| requirement(ViolationKind::Pattern(ARN_PATTERN.to_string())))
}

/// The refusal for an ARN-typed member shorter than the [`MIN_ARN_CHARS`] an ARN takes:
/// the one-violation constraint clause naming the member and that minimum, which is what
/// tooling reading a refusal parses out of it. `None` for a value long enough to be an ARN.
pub fn short_arn_message(member: &str, value: &str) -> Option<String> {
    use ::aws_smithy_schema_server::constraints::ViolationKind;
    (value.chars().count() < MIN_ARN_CHARS).then(|| {
        validation_message(
            member,
            &::service_common::validation_message::requirement(ViolationKind::MinLength(
                as_bound(MIN_ARN_CHARS),
            )),
        )
    })
}

/// The refusal for an `Arn`-typed member of the S3-Table-Integration family this service
/// cannot read an ARN out of: too short to be one at all ([`short_arn_message`]), and
/// otherwise long enough but unparsable ([`unparsable_arn_message`]). `None` when the value
/// parses as an ARN, whatever resource it names.
pub fn malformed_arn_message(member: &str, value: &str) -> Option<String> {
    short_arn_message(member, value).or_else(|| unparsable_arn_message(member, value))
}

/// The resource an S3 Table Integration's ARN names, read from the service that OWNS one
/// rather than spelled again here: `s3tableintegration/{integration-id}`, the form
/// `ListSourcesForS3TableIntegration` states when it is handed an ARN of another resource.
pub const S3_TABLE_INTEGRATION_RESOURCE_PREFIX: &str =
    ::service_com_amazonaws_observabilityadmin_observabilityadmin::state::S3_TABLE_INTEGRATION_ARN_RESOURCE_PREFIX;

/// The ARN of the S3 Table Integration `id` in `region`: the handle the service that OWNS
/// one mints for it, which is the only form an `integrationArn` of this family names an
/// integration by — built by that service's own ARN function, so the two cannot drift.
///
/// An integration is reachable only from the account and Region it was created in, so the
/// partition, the Region and the account are part of the handle and not decoration: an ARN
/// differing in any of them names no integration this account can associate a source with.
pub fn s3_table_integration_arn(region: &str, id: &str) -> String {
    ::service_com_amazonaws_observabilityadmin_observabilityadmin::state::s3_table_integration_arn(
        region, id,
    )
}

/// Whether `arn`'s RESOURCE is the one an S3 Table Integration's ARN carries:
/// `s3tableintegration/{integration-id}`, with a non-empty id. The form the reader states
/// when it is handed an ARN of another resource type
/// ([`not_an_s3_table_integration_arn_message`]).
pub fn names_an_s3_table_integration_resource(arn: &str) -> bool {
    arn_service_and_resource(arn)
        .and_then(|(_service, resource)| {
            resource.strip_prefix(S3_TABLE_INTEGRATION_RESOURCE_PREFIX)
        })
        .is_some_and(|id| !id.is_empty())
}

/// Whether `arn` is the HANDLE of an S3 Table Integration a caller signed for `region` could
/// hold: an ARN the owning service could have minted there for this account
/// ([`s3_table_integration_arn`]), carrying a non-empty integration id. Whether the account
/// actually holds it is a further question ([`holds_s3_table_integration`]).
pub fn names_an_s3_table_integration(region: &str, arn: &str) -> bool {
    arn_service_and_resource(arn)
        .and_then(|(_service, resource)| {
            resource.strip_prefix(S3_TABLE_INTEGRATION_RESOURCE_PREFIX)
        })
        .is_some_and(|id| !id.is_empty() && arn == s3_table_integration_arn(region, id))
}

/// Whether the account HOLDS the S3 Table Integration `arn` names, as of a request signed
/// for `region`.
///
/// An S3 Table Integration is a record of the service that owns them, which this service
/// reads through the shared engine rather than keeping a second reading of which
/// integrations exist: an integration that service does not hold is one no source can be
/// given query access through. An emulator that is not serving that service holds none.
pub fn holds_s3_table_integration<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    region: &str,
    arn: &str,
) -> bool {
    ::service_com_amazonaws_observabilityadmin_observabilityadmin::state::service_state(engine)
        .is_some_and(|state| state.lock().s3_table_integration(region, arn).is_some())
}

/// The refusal for an `integrationArn` whose RESOURCE is not an integration's: the fixed
/// sentence stating the resource form an integration's ARN carries, followed by the ARN the
/// request gave — the one variable part, so a caller can see which of its ARNs was in the
/// wrong form.
pub fn not_an_s3_table_integration_arn_message(arn: &str) -> Option<String> {
    (!names_an_s3_table_integration_resource(arn)).then(|| {
        format!(
            "Invalid S3 table integration ARN format. Expected: \
             {S3_TABLE_INTEGRATION_RESOURCE_PREFIX}{{integration-id}}: {arn}"
        )
    })
}

/// The refusal for an `integrationArn` naming an integration of ANOTHER account: query
/// access is given through an integration of the CALLER's own account, and an ARN carrying
/// another account's id is refused by stating that limit and the ARN that named it. `None`
/// for an ARN of this account, and for one that names no account at all.
pub fn cross_account_integration_message(arn: &str) -> Option<String> {
    let account = ::service_common::arn::parse(arn)?.account;
    (!account.is_empty() && account != ::engine::emulator_account_id()).then(|| {
        format!("Cross account access not supported for S3 Table Integrations: {arn}")
    })
}

/// The refusal for an `integrationArn` that is not the handle of any integration this caller
/// could hold — an ARN of another service, partition or Region
/// ([`names_an_s3_table_integration`]): the fixed text and the ARN it named.
pub fn invalid_integration_arn_message(region: &str, arn: &str) -> Option<String> {
    (!names_an_s3_table_integration(region, arn)).then(|| format!("Invalid integration ARN: {arn}"))
}

/// The refusal for an `integrationArn` that IS such a handle and names an integration the
/// account does not hold: the fixed text and the ARN it named.
pub fn integration_not_found_arn_message(arn: &str) -> String {
    format!("Integration not found: {arn}")
}

/// The resource an INTEGRATION OF THIS SERVICE carries in its ARN:
/// `integration:{integrationName}` — the resource `PutIntegration` establishes, `GetIntegration`
/// reads and `DeleteIntegration` removes — under this service's own ARN vendor.
const LOGS_INTEGRATION_RESOURCE_PREFIX: &str = "integration:";
/// See [`LOGS_INTEGRATION_RESOURCE_PREFIX`].
const LOGS_ARN_VENDOR: &str = "logs";

/// The name of the integration OF THIS SERVICE that `arn` names, or `None` for an ARN of any
/// other resource.
fn logs_integration_name(arn: &str) -> Option<&str> {
    let (service, resource) = arn_service_and_resource(arn)?;
    if service != LOGS_ARN_VENDOR {
        return None;
    }
    resource
        .strip_prefix(LOGS_INTEGRATION_RESOURCE_PREFIX)
        .filter(|name| !name.is_empty())
}

/// The refusal for an `integrationArn` that names an integration OF THIS SERVICE the account
/// does not hold, or `None` for an ARN of another resource and for one naming an integration
/// the account does hold.
///
/// An ARN of this form names an INTEGRATION — the resource this service's own integration
/// operations establish — so addressing one that was never created, or one that has been
/// deleted, is being asked about a resource that does not exist rather than stating the member
/// incorrectly: the NOT-FOUND class, worded as this service words a missing integration
/// ([`integration_not_found_arn_message`]). An integration the account DOES hold under that
/// name is a real resource of the wrong KIND for this family, which the resource-form rung
/// states ([`not_an_s3_table_integration_arn_message`]).
fn unheld_logs_integration_message<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    arn: &str,
) -> Option<String> {
    let name = logs_integration_name(arn)?;
    let held = super::service_state(engine)
        .is_some_and(|state| state.lock().integrations.contains_key(name));
    (!held).then(|| integration_not_found_arn_message(arn))
}

/// WHY an `integrationArn` resolved to no S3 Table Integration, which is what decides the
/// CLASS the refusal travels under: an ARN that names no integration of this account is a
/// parameter stated incorrectly, while one that IS such a handle and names an integration the
/// account does not hold is a resource that does not exist ([`unresolved_integration`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnresolvedIntegration {
    /// The ARN could name no integration this caller holds: another resource type, another
    /// account, or another service, partition or Region.
    Invalid(String),
    /// The ARN is the handle of an integration this caller could hold, and the account holds
    /// none under it.
    NotFound(String),
}

impl UnresolvedIntegration {
    /// The sentence this refusal states.
    #[must_use]
    pub fn message(self) -> String {
        match self {
            Self::Invalid(message) | Self::NotFound(message) => message,
        }
    }
}

/// The refusal for an `integrationArn` that could name NO S3 Table Integration of this
/// account, whatever the account holds: a resource that is not an integration's is told the
/// form one carries, another account's integration is told that access is not given across
/// accounts, and an ARN of another service, partition or Region is told it names no
/// integration. `None` for an ARN that IS such a handle — whether or not the account holds
/// the integration it names.
///
/// The rungs that judge the VALUE, in one place, so every operation of this family reads an
/// `integrationArn` the same way. The value has already been read as an ARN by the caller
/// ([`malformed_arn_message`]), which is what the length and the parse are stated by.
pub fn invalid_s3_table_integration_arn_message(region: &str, arn: &str) -> Option<String> {
    not_an_s3_table_integration_arn_message(arn)
        .or_else(|| cross_account_integration_message(arn))
        .or_else(|| invalid_integration_arn_message(region, arn))
}

/// The refusal `ListSourcesForS3TableIntegration` answers for an `integrationArn` it cannot
/// resolve an S3 Table Integration from, or `None` when the account holds the one it names.
///
/// This reader resolves ONE family — the S3 Table Integrations of the service that owns them,
/// which is what its `integrationArn` names — so an ARN of any other resource is told the
/// form an integration's carries ([`invalid_s3_table_integration_arn_message`]), including an
/// ARN naming an integration of THIS service, which is a different resource and not a
/// coarser-grained one. A handle of the right form the account holds nothing under is told the
/// integration is not there.
///
/// Every rung of this reader's ladder travels under ONE class, the modelled
/// `ValidationException`, which is RECORDED for this operation rather than derived from the
/// rung: fixture `logs-b2-lookup-tables-and-integrations` step `task#6` answers an ARN of this
/// service's own `integration:` resource `400 ValidationException` with the resource-form
/// sentence, and fixture `logs-s3table-integration-not-found-workflow` step `task#2` answers a
/// well-formed handle the account holds nothing under `400 ValidationException` with
/// [`integration_not_found_arn_message`]. The associate words its own classes
/// ([`unresolved_integration`]).
pub fn unresolved_s3_table_integration_message<
    Eng: ::std::marker::Send + ::std::marker::Sync + 'static,
>(
    engine: &Eng,
    region: &str,
    arn: &str,
) -> Option<String> {
    invalid_s3_table_integration_arn_message(region, arn).or_else(|| {
        (!holds_s3_table_integration(engine, region, arn))
            .then(|| integration_not_found_arn_message(arn))
    })
}

/// The refusal `AssociateSourceToS3TableIntegration` answers for an `integrationArn` it cannot
/// resolve an S3 Table Integration from, or `None` when the account holds the one it names.
///
/// The rungs are the reader's ([`unresolved_s3_table_integration_message`]) plus the one the
/// associate has of its own: an integration of THIS service's own that the account does not
/// hold is told the integration is not there. Each rung is worded as this service words it.
///
/// The rungs that say an INTEGRATION is not there are the NOT-FOUND class and the rest are the
/// parameter-validity one: an ARN that names an integration — of either family this service
/// resolves one from — is addressing a resource, and an account that holds none under it is
/// being asked about one that does not exist, which is the class `ResourceNotFoundException`
/// ("the specified resource does not exist") carries and which every operation of this family
/// models. An ARN that could name no integration at all states the member incorrectly.
///
/// The value has already been read as an ARN by the caller
/// ([`malformed_arn_message`]), which is what the length and the parse are stated by.
pub fn unresolved_integration<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    region: &str,
    arn: &str,
) -> Option<UnresolvedIntegration> {
    if let Some(message) = unheld_logs_integration_message(engine, arn) {
        return Some(UnresolvedIntegration::NotFound(message));
    }
    if let Some(message) = invalid_s3_table_integration_arn_message(region, arn) {
        return Some(UnresolvedIntegration::Invalid(message));
    }
    (!holds_s3_table_integration(engine, region, arn))
        .then(|| UnresolvedIntegration::NotFound(integration_not_found_arn_message(arn)))
}

/// The fixed sentences CloudWatch Logs refuses a data source whose NAME or whose TYPE
/// carries a character a source cannot hold with. Each states the RULE and carries no
/// variable part — nothing of the submitted value is reflected back.
pub const DATA_SOURCE_NAME_MESSAGE: &str =
    "Source name must only contain letters, numbers, and underscores";
/// See [`DATA_SOURCE_NAME_MESSAGE`]. A data source TYPE is held to the LOWER-CASE spelling
/// ([`is_data_source_type`]), which is what "letters" means for a type: the service states
/// this one sentence for either fault.
pub const DATA_SOURCE_TYPE_MESSAGE: &str =
    "Source type must only contain letters, numbers, and underscores";

/// Whether `value` is spelled in the characters a data source's NAME is spelled in: letters,
/// digits and underscores — the character set the names of this service's own data sources use
/// (`amazon_vpc`, `aws_cloudtrail`).
fn is_data_source_word(value: &str) -> bool {
    value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Whether `value` is a data source TYPE: the same word characters, spelled in LOWER CASE.
///
/// A type is not a free label a caller picks the way a name is — it "categorizes logs by
/// originating service and log type" (`DataSource`), and the types this service states for its
/// own data sources are lower-case words (`flow`, beside the source names `amazon_vpc` and
/// `aws_cloudtrail`). Recorded: one and the same type is associated when it is spelled in
/// lower case and refused when the identical word is spelled in capitals — which is the
/// spelling a vended-log `logType` uses (`APPLICATION_LOGS`), a different catalogue from this
/// one.
fn is_data_source_type(value: &str) -> bool {
    value.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
    })
}

/// The refusal for a `dataSource` whose NAME carries anything but those characters
/// ([`DATA_SOURCE_NAME_MESSAGE`]). `None` for a name inside that set.
pub fn invalid_data_source_name_message(name: &str) -> Option<&'static str> {
    (!is_data_source_word(name)).then_some(DATA_SOURCE_NAME_MESSAGE)
}

/// The refusal for a `dataSource` whose TYPE is not spelled the way a data source type is
/// ([`is_data_source_type`], [`DATA_SOURCE_TYPE_MESSAGE`]). `None` for a type that is, and for
/// a request that names no type at all ([`missing_data_source_type_message`] is what states
/// that).
pub fn invalid_data_source_type_message(source_type: Option<&str>) -> Option<&'static str> {
    source_type
        .filter(|source_type| !is_data_source_type(source_type))
        .map(|_| DATA_SOURCE_TYPE_MESSAGE)
}

/// The position the service validates the TYPE of an associate's data source at. The request
/// spells that member `dataSource.type`; the refusal names the position the service holds it
/// at, which is the one a caller reads out of the message.
const DATA_SOURCE_TYPE_MEMBER: &str = "sourceType";

/// The refusal for an associate whose `dataSource` names no TYPE: the service requires one —
/// a data source "categorizes logs by originating service and log type", so a source with no
/// type categorizes nothing.
///
/// Both the requirement and the position it is stated at are recorded behavior: the service
/// refuses an associate carrying a `dataSource` with no type, and names that position
/// `sourceType` rather than the `dataSource.type` path the request spells the member under.
/// The clause and the envelope are the crate's single wording layer's
/// ([`crate::service::absent_member_clause`], which is where the `Value null at` subject this
/// one refusal states is documented).
pub fn missing_data_source_type_message() -> String {
    let clause = crate::service::absent_member_clause(
        DATA_SOURCE_TYPE_MEMBER,
        "Member must not be null",
    );
    ::service_common::validation_message::envelope_over(::std::slice::from_ref(&clause))
        .unwrap_or(clause)
}

/// The requirement a `retentionDays` outside the bounds the service holds it to
/// ([`MIN_OPENSEARCH_RETENTION_DAYS`]..[`MAX_OPENSEARCH_RETENTION_DAYS`]) failed, or `None`
/// for a value inside them.
fn retention_days_requirement(retention_days: i32) -> Option<String> {
    if retention_days < MIN_OPENSEARCH_RETENTION_DAYS {
        return Some(format!(
            "Member must have value greater than or equal to {MIN_OPENSEARCH_RETENTION_DAYS}"
        ));
    }
    if retention_days > MAX_OPENSEARCH_RETENTION_DAYS {
        return Some(format!(
            "Member must have value less than or equal to {MAX_OPENSEARCH_RETENTION_DAYS}"
        ));
    }
    None
}

/// The input position a clause names one of a `PutIntegration` configuration's members at:
/// the path from the request inwards, through the `resourceConfig` union's OpenSearch
/// variant, which is how the modelled walk spells a member nested inside a union and a
/// structure.
fn config_member_path(member: &str) -> String {
    format!("resourceConfig.openSearchResourceConfig.{member}")
}

/// The constraint set a clause states for the ELEMENTS of a list of ARNs: a collection's
/// clause states what its elements have to satisfy rather than what one element happened to
/// break, so the whole set is nested inside the element-set wrapper every service words one
/// with.
///
/// The set is the one this service holds every `Arn`-typed member to ([`arn_constraint_set`]),
/// in the order that function states it, so the requirements a list member's clause lists are
/// the requirements a single ARN member is judged by one at a time.
fn arn_element_set() -> String {
    ::service_common::validation_message::element_set_requirement(&arn_constraint_set())
}

/// The refusal for a `PutIntegration` configuration whose members break the constraints the
/// SERVICE holds them to: the constraint set of an ARN on every `Arn`-typed member — an ARN
/// member is a constrained string before it is a reference to a resource — and the retention
/// bounds on `retentionDays`.
///
/// EVERY member that breaks one is reported, in one envelope, each clause naming the
/// member's own path and the requirement it failed; the list member states the element
/// shape's whole set ([`arn_element_set`]), which is how a collection's clause states one.
/// The clauses are stated in the ascending order of the member names, which is the order the
/// service counts them in. `None` when the configuration breaks none.
pub fn config_constraint_envelope(
    config: &crate::ops::types::OpenSearchResourceConfig,
) -> Option<String> {
    let mut refused: Vec<(&str, String)> = Vec::new();
    // `dashboardViewerPrincipals` is a LIST of ARNs: its clause states its ELEMENTS'
    // constraint set, once however many of them failed however many of its constraints.
    if config
        .dashboard_viewer_principals
        .iter()
        .any(|principal| arn_requirement(principal).is_some())
    {
        refused.push(("dashboardViewerPrincipals", arn_element_set()));
    }
    for (member, value) in [
        ("applicationArn", config.application_arn.as_deref()),
        (
            "dataSourceRoleArn",
            Some(config.data_source_role_arn.as_str()),
        ),
        ("kmsKeyArn", config.kms_key_arn.as_deref()),
    ] {
        if let Some(requirement) = value.and_then(arn_requirement) {
            refused.push((member, requirement));
        }
    }
    if let Some(requirement) = retention_days_requirement(config.retention_days) {
        refused.push(("retentionDays", requirement));
    }
    refused.sort_by_key(|(member, _)| *member);
    let clauses: Vec<String> = refused
        .into_iter()
        .map(|(member, requirement)| {
            crate::service::clause_at(&config_member_path(member), &requirement)
        })
        .collect();
    ::service_common::validation_message::envelope_over(&clauses)
}

/// The days of retention `OpenSearchResourceConfig$retentionDays` admits at least.
pub const MIN_OPENSEARCH_RETENTION_DAYS: i32 = 1;

/// The days of retention `OpenSearchResourceConfig$retentionDays` admits at most: ten years,
/// the bound the SERVICE enforces and states in its refusal. The model's own `@range` on that
/// member stops at 30, which the service does not — a retention of several hundred days is
/// accepted — so the member carries `skip_validation` and this bound is applied here
/// ([`crate::ops::types::OpenSearchResourceConfig`]).
pub const MAX_OPENSEARCH_RETENTION_DAYS: i32 = 3650;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an `Arn`-typed member of this family is held to the minimum length an ARN
    /// takes before anything is parsed out of it, and only a value long enough to be an ARN
    /// is refused as unparsable.
    #[test]
    fn an_arn_member_is_held_to_its_minimum_length_before_it_is_parsed() {
        let twenty = "arn:aws:logs:::aaaa";
        assert_eq!(
            twenty.chars().count(),
            MIN_ARN_CHARS - 1,
            "the case below is one character short of the bound"
        );
        // (case, the value, the refusal expected for it)
        let cases: [(&str, &str, Option<String>); 4] = [
            (
                "a value far shorter than an ARN",
                "arn:aws",
                Some(validation_message(
                    "integrationArn",
                    "Member must have length greater than or equal to 20",
                )),
            ),
            (
                "one character short of the bound",
                twenty,
                Some(validation_message(
                    "integrationArn",
                    "Member must have length greater than or equal to 20",
                )),
            ),
            (
                "long enough, but not an ARN",
                "not-an-arn-but-long-enough",
                Some(
                    "1 validation error detected: Unable to parse integrationArn \
                     not-an-arn-but-long-enough"
                        .to_string(),
                ),
            ),
            (
                "an ARN",
                "arn:aws:logs:eu-west-2:000000000000:s3tableintegration/abc",
                None,
            ),
        ];
        for (case, value, expected) in cases {
            let actual = malformed_arn_message("integrationArn", value);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
