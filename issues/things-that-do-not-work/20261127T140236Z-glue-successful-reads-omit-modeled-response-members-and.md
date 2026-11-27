Issue-Source: trace-sweep-divergence
Issue-Trace: glue-cancel-data-quality-rule-recommendation-run-refused-ids-and-stopped-recancel
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-cancel-data-quality-rule-recommendation-run-refused-ids-and-stopped-recancel glue-start-data-quality-rule-recommendation-run-cancel-and-validation glue-crawler-metrics-and-schedule-management glue-job-runs-cursor-from-another-listing-refused glue-partition-batch-and-page-size-limits glue-connection-type-registration-lifecycle
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|CancelDataQualityRuleRecommendationRun|2xx->2xx|$
Issue-Key: glue|GetDataQualityRuleRecommendationRun|2xx->2xx|$.DataSource.GlueTable.CatalogId
Issue-Key: glue|GetDataQualityRuleRecommendationRun|2xx->2xx|$.DataSource.GlueTable.__type
Issue-Key: glue|GetCrawlerMetrics|2xx->2xx|$.CrawlerMetricsList[*].AssetsCreated
Issue-Key: glue|GetCrawlerMetrics|2xx->2xx|$.CrawlerMetricsList[*].AssetsDeleted
Issue-Key: glue|GetCrawlerMetrics|2xx->2xx|$.CrawlerMetricsList[*].AssetsUpdated
Issue-Key: glue|GetJobRuns|2xx->2xx|$.NextToken
Issue-Key: glue|GetPartitions|2xx->2xx|$.NextToken
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.AuthenticationConfiguration.AuthenticationType.AllowedValues[*].Description
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.AuthenticationConfiguration.AuthenticationType.AllowedValues[*].DisplayName
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.AuthenticationConfiguration.AuthenticationType.DataType
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.AuthenticationConfiguration.AuthenticationType.Description
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.AuthenticationConfiguration.AuthenticationType.DisplayName
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.AuthenticationConfiguration.AuthenticationType.Phase
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.AuthenticationConfiguration.AuthenticationType.PropertyType
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.AuthenticationConfiguration.AuthenticationType.Validations
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.AuthenticationConfiguration.BasicAuthenticationProperties
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.AuthenticationConfiguration.SecretArn
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.AuthenticationConfiguration.__type
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.Capabilities.SupportedComputeEnvironments[*]
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.Capabilities.SupportsCatalogFederation
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.Capabilities.SupportsCustomHttpProxy
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.Capabilities.SupportsResourceRetrieval
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.Capabilities.SupportsServiceManagedAuthorization
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.Capabilities.__type
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionOptions.ENTITY_NAME.DataOperationScopes
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionOptions.ENTITY_NAME.DataType
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionOptions.ENTITY_NAME.Description
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionOptions.ENTITY_NAME.DisplayName
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionOptions.ENTITY_NAME.PropertyType
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionOptions.ENTITY_NAME.Validations
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionOptions.ENTITY_NAME.__type
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionProperties.ROLE_ARN
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionProperties.URL.DataType
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionProperties.URL.Description
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionProperties.URL.DisplayName
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionProperties.URL.Name
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionProperties.URL.Phase
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionProperties.URL.PropertyType
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionProperties.URL.Validations
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectionProperties.URL.__type
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.ConnectorAuthenticationConfiguration
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.AvailabilityZone.DataType
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.AvailabilityZone.Description
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.AvailabilityZone.DisplayName
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.AvailabilityZone.PropertyType
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.AvailabilityZone.Validations
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.AvailabilityZone.__type
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.SecurityGroupIdList.DataType
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.SecurityGroupIdList.Description
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.SecurityGroupIdList.DisplayName
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.SecurityGroupIdList.PropertyType
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.SecurityGroupIdList.Validations
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.SecurityGroupIdList.__type
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.SubnetId.DataType
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.SubnetId.Description
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.SubnetId.DisplayName
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.SubnetId.PropertyType
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.SubnetId.Validations
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.PhysicalConnectionRequirements.SubnetId.__type
Issue-Key: glue|DescribeConnectionType|2xx->2xx|$.RestConfiguration.__type
Issue-Related: 20261127T055032Z-glue-synthesized-read-documents-diverge-from-aws-getplan.md
Issue-Commit: bb517dad8aa2a6dcfbb56aa45c4ce46fe44a83f5

# Glue successful reads omit modeled response members and pagination NextToken

> _Note: this issue was identified against the code base at commit `bb517dad8aa2a6dcfbb56aa45c4ce46fe44a83f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelDataQualityRuleRecommendationRun

## Description
Six net-new root causes in glue: the call succeeds with HTTP 200 but the serialized body is missing members AWS sends — including the pagination cursor, which silently truncates two paginated listings.

Cause 1 of 6: CancelDataQualityRuleRecommendationRun answers an empty body instead of echoing the cancelled run.
AWS: the cancel call answers 200 with the run's `RunId` (the id from the request) and its resulting `Status` (a TaskStatusType value). The checkout's Smithy models CancelDataQualityRuleRecommendationRunResponse with no members, so this is recorded behavior only: the recording shows both members present on the success response.
Emulator: 200 with `{}` — no RunId, no Status.
Why: a caller has no confirmation of which run was cancelled or what state it landed in, and must issue an extra Get to learn the status AWS already returned; code reading response['Status'] raises instead.
Fix: the output serializer of the CancelDataQualityRuleRecommendationRun handler in the glue service crate (populate RunId and the post-cancel Status from the run's state).
Traces: glue-cancel-data-quality-rule-recommendation-run-refused-ids-and-stopped-recancel#task#12.

Cause 2 of 6: GetDataQualityRuleRecommendationRun's echoed DataSource.GlueTable omits CatalogId and the nested `__type` discriminator.
AWS: the read answers 200 with `DataSource` echoing the run's GlueTable; `GlueTable.CatalogId` is a modeled member (Smithy: GlueTable{DatabaseName required, TableName required, CatalogId, ConnectionName, AdditionalOptions}) and is resolved to the catalog the table was read under (the caller's account id) even when the start request omitted it. The awsJson1.1 serializer also writes the `__type` tag naming the shape onto this nested structure (Smithy: ErrorDetail.__type documents that tag as what this service's awsJson1.1 serializer writes onto nested structures so a client dispatching on it can identify the shape).
Emulator: `DataSource.GlueTable` carries neither `CatalogId` nor `__type`.
Why: a client that resolves the run's table through the echoed DataSource has no catalog id to address it with and falls back to a guess, and a client dispatching on the nested type tag cannot identify the structure.
Fix: the DataSource/GlueTable serializer used by the GetDataQualityRuleRecommendationRun handler (default CatalogId to the resolved catalog, and emit the nested `__type` tag).
Traces: glue-cancel-data-quality-rule-recommendation-run-refused-ids-and-stopped-recancel#task#13.

Cause 3 of 6: GetCrawlerMetrics omits the per-crawler asset counters.
AWS: GetCrawlerMetrics answers 200 with CrawlerMetricsList, and each entry reports asset counters `AssetsCreated`, `AssetsUpdated` and `AssetsDeleted` alongside the table counters, defaulting to 0 for a crawler that has not run. The checkout's Smithy CrawlerMetrics models only CrawlerName/TimeLeftSeconds/StillEstimating/LastRuntimeSeconds/MedianRuntimeSeconds/TablesCreated/TablesUpdated/TablesDeleted, so the three Assets* members are recorded behavior only: the recording shows them present with value 0.
Emulator: the entry carries the modeled members but no AssetsCreated/AssetsUpdated/AssetsDeleted at all.
Why: a client reading the asset counters (the newer superset of the table counters) gets a KeyError/None where AWS always reports a number, so crawler progress dashboards show no asset activity.
Fix: the CrawlerMetrics serializer in the GetCrawlerMetrics handler of the glue service crate (emit the three asset counters, zero when the crawler has produced nothing).
Traces: glue-crawler-metrics-and-schedule-management#task#6.

Cause 4 of 6: GetJobRuns does not return a NextToken when the page it served does not exhaust the run list.
AWS: GetJobRuns is `@paginated` with inputToken/outputToken NextToken, items JobRuns and pageSize MaxResults (Smithy). When MaxResults is smaller than the number of runs for the job, the response carries a NextToken that continues the listing; the token's value is opaque and environment-specific, but its PRESENCE whenever more runs remain is the requirement.
Emulator: the truncated page is served with JobRuns but no NextToken.
Why: every paginator stops after the first page, so a client silently sees only part of the job's run history and treats it as complete — a data-loss-shaped divergence, not a cosmetic one.
Fix: the pagination state/cursor emission in the GetJobRuns handler of the glue service crate.
Traces: glue-job-runs-cursor-from-another-listing-refused#task#17.

Cause 5 of 6: GetPartitions does not return a NextToken when the page it served does not exhaust the partition list.
AWS: GetPartitions is `@paginated` with inputToken/outputToken NextToken and pageSize MaxResults (Smithy). A MaxResults below the partition count yields a page plus a NextToken; again the token value is opaque, its presence is the requirement.
Emulator: the truncated page is served with Partitions but no NextToken.
Why: as Cause 4 — partition enumeration terminates early and a client under-reads the table's partitions. (Cause 4 and Cause 5 are separate handlers; the shared paginator is the likely common root.)
Fix: the pagination state/cursor emission in the GetPartitions handler of the glue service crate.
Traces: glue-partition-batch-and-page-size-limits#task#8.

Cause 6 of 6: DescribeConnectionType's document for a connection type is a reduced, re-worded stand-in for AWS's property catalog.
AWS: DescribeConnectionType answers 200 with the full property catalog for the type: Description, Capabilities, ConnectionProperties, ConnectionOptions, AuthenticationConfiguration, ComputeEnvironmentConfigurations, PhysicalConnectionRequirements and RestConfiguration (Smithy: DescribeConnectionType output). Each property entry is a full Property structure (DataType, DisplayName, Description, Name, Phase, PropertyType/PropertyTypes, Required, Validations, and for auth entries KeyOverride/PropertyLocation and the per-grant-type ConditionallyRequired sets), the AuthenticationConfiguration carries its AuthenticationType AllowedValues plus BasicAuthenticationProperties/SecretArn, Capabilities carries the full supported-compute/federation/proxy/retrieval/service-managed-authorization flags, and every nested structure carries its awsJson1.1 `__type` tag.
Emulator: the response omits most of those members (DisplayName, LogoUrl, Categories, ConnectorAuthenticationConfiguration, the Property sub-members, the Capabilities flags, every nested `__type`) and replaces the descriptions it does serve with its own prose — e.g. AuthenticationType.Description is 'The kind of credentials this connection authenticates with.' where AWS sends 'Authentication type to connect to data source.', and its BASIC AllowedValue description is 'Authentication with the user name and password the data source is reached as.' where AWS sends 'BASIC Authentication'.
Why: this document is exactly what a console or a connection wizard renders and validates user input against; a client driven by it prompts for the wrong fields, cannot tell which are Required or SECRET, and shows text no AWS region serves.
Fix: the connection-type catalog data behind the DescribeConnectionType handler of the glue service crate (the per-type property documents must carry AWS's member set and AWS's descriptions, with `__type` on nested structures).
Traces: glue-connection-type-registration-lifecycle#task#2.
Note: 20261127T055032Z-glue-synthesized-read-documents-diverge-from-aws-getplan.md owns the DescribeConnectionType divergence seen in glue-connection-type-catalog#task#2; it carries no replay key for this registration-lifecycle step, whose diff is the distinct 'registered type described after RegisterConnectionType' path.

## Evidence
Cause 1 — fixture glue-cancel-data-quality-rule-recommendation-run-refused-ids-and-stopped-recancel step task#12, expected 200 / actual 200:
  $: empty_body (expected {'RunId': '<run id>', 'Status': '<shaped-not-asserted>'}, got {})
  Also at glue-cancel-data-quality-rule-recommendation-run-refused-ids-and-stopped-recancel#task#14 and glue-start-data-quality-rule-recommendation-run-cancel-and-validation#task#8.

Cause 2 — same fixture, step task#13 (GetDataQualityRuleRecommendationRun), expected 200 / actual 200:
  $.DataSource.GlueTable.__type: missing (expected 'GlueTable', got None)
  $.DataSource.GlueTable.CatalogId: missing (expected the caller's catalog id, got None)

Cause 3 — fixture glue-crawler-metrics-and-schedule-management step task#6 (GetCrawlerMetrics), expected 200 / actual 200:
  $.CrawlerMetricsList[0].AssetsCreated: missing (expected 0, got None)
  $.CrawlerMetricsList[0].AssetsDeleted: missing (expected 0, got None)
  $.CrawlerMetricsList[0].AssetsUpdated: missing (expected 0, got None)

Cause 4 — fixture glue-job-runs-cursor-from-another-listing-refused step task#17 (GetJobRuns), expected 200 / actual 200:
  $.NextToken: missing (expected '<shaped-not-asserted>', got None)

Cause 5 — fixture glue-partition-batch-and-page-size-limits step task#8 (GetPartitions), expected 200 / actual 200:
  $.NextToken: missing (expected '<shaped-not-asserted>', got None)

Cause 6 — fixture glue-connection-type-registration-lifecycle step task#2 (DescribeConnectionType), expected 200 / actual 200: 50+ diverging keys, including
  $.AuthenticationConfiguration.__type: missing (expected 'AuthConfiguration', got None)
  $.AuthenticationConfiguration.AuthenticationType.AllowedValues[0].Description: value_mismatch (expected 'BASIC Authentication', got 'Authentication with the user name and password the data source is reached as.')
  $.AuthenticationConfiguration.AuthenticationType.{DataType,DisplayName,Phase,PropertyType,Validations}: missing
  $.AuthenticationConfiguration.BasicAuthenticationProperties / .SecretArn: missing
  $.Capabilities.{SupportsCatalogFederation,SupportsCustomHttpProxy,SupportsResourceRetrieval,SupportsServiceManagedAuthorization,__type}: missing
  $.ConnectionOptions.ENTITY_NAME.{DataType,Description,DisplayName,PropertyType,Validations,__type}: missing
  $.PhysicalConnectionRequirements.{AvailabilityZone,SecurityGroupIdList,SubnetId}.*: missing
  $.{Categories,DisplayName,LogoUrl,ConnectorAuthenticationConfiguration,RestConfiguration.__type}: missing

## Steps to Reproduce
Cause 1: StartDataQualityRuleRecommendationRun, then CancelDataQualityRuleRecommendationRun(RunId) and read the 200 body.
Cause 2: the same run, then GetDataQualityRuleRecommendationRun(RunId) and read $.DataSource.GlueTable.
Cause 3: CreateCrawler, then GetCrawlerMetrics(CrawlerNameList=[name]) and read CrawlerMetricsList[0].
Cause 4: CreateJob, StartJobRun several times, then GetJobRuns(JobName, MaxResults smaller than the run count) and look for NextToken.
Cause 5: CreateDatabase/CreateTable with PartitionKeys, CreatePartition several times, then GetPartitions(MaxResults smaller than the partition count) and look for NextToken.
Cause 6: RegisterConnectionType a REST- type, then DescribeConnectionType on it and compare the property catalog member-by-member.
