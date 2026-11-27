Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-create-data-source-relational-source-type-enum
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-create-data-source-relational-source-type-enum appsync-data-source-type-enum-refusal appsync-http-data-source-authorization-type-enum appsync-function-runtime-name-enum-rejection appsync-function-sync-config-conflict-handler-enum-rejection appsync-list-graphql-apis-filter-enum-rejections appsync-resolver-metrics-config-enum-rejection appsync-source-association-merge-type-enum appsync-update-association-merge-type-outside-enum appsync-update-data-source-metrics-config-enum appsync-update-data-source-type-outside-enum appsync-update-graphql-api-data-source-metrics-enum appsync-update-graphql-api-resolver-metrics-behavior-enum appsync-update-resolver-kind-outside-enum appsync-update-resolver-metrics-config-enum
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|CreateDataSource|4xx->4xx|$.message
Issue-Key: appsync|CreateDataSource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|CreateFunction|4xx->4xx|$.message
Issue-Key: appsync|CreateFunction|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|ListGraphqlApis|4xx->4xx|$.message
Issue-Key: appsync|ListGraphqlApis|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|CreateResolver|4xx->4xx|$.message
Issue-Key: appsync|CreateResolver|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|AssociateSourceGraphqlApi|4xx->4xx|$.message
Issue-Key: appsync|AssociateSourceGraphqlApi|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|UpdateSourceApiAssociation|4xx->4xx|$.message
Issue-Key: appsync|UpdateSourceApiAssociation|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|UpdateDataSource|4xx->4xx|$.message
Issue-Key: appsync|UpdateDataSource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|UpdateGraphqlApi|4xx->4xx|$.message
Issue-Key: appsync|UpdateGraphqlApi|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|UpdateResolver|4xx->4xx|$.message
Issue-Key: appsync|UpdateResolver|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 5d024fed191bbfdb41e9accab576b1e1cc16d383

# AppSync enum-member refusals use an unmodeled ValidationException and generic constraint text instead of AppSync's refusal messages

> _Note: this issue was identified against the code base at commit `5d024fed191bbfdb41e9accab576b1e1cc16d383`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataSource

## Description
Whenever a request carries a value outside a modeled AppSync enum, the emulator answers the generic Smithy frame "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [...]" and tags it x-amzn-errortype: ValidationException. AWS words each of these refusals itself, and the appsync Smithy model binds NO ValidationException shape anywhere — the service's 400 client error is BadRequestException — so both the message and the error class are wrong on every one of these paths. The status (400) is correct throughout. 10 causes in this issue; all ten must be closed before submitting. The open issue 20261029T174359Z-appsync-control-plane-error-semantics-not-found-bad.md names a ValidationException-for-BadRequestException defect, but it carries none of the replay keys below (its grounding is a different, behavioral check set), so none of these causes is covered by it.

=== Cause 1 of 10: CreateDataSource relationalDatabaseConfig.relationalDatabaseSourceType ===
AWS: Smithy (appsync) CreateDataSource, RelationalDatabaseDataSourceConfig$relationalDatabaseSourceType: enum RelationalDatabaseSourceType (RDS_HTTP_ENDPOINT); modeled BadRequestException|400. Recorded behavior for a value outside the enum is 400 with the fixed message 'Invalid relational database source type' — entirely fixed text, no request values.
Emulator: "1 validation error detected: Value at 'relationalDatabaseConfig.relationalDatabaseSourceType' failed to satisfy constraint: Member must satisfy enum value set: [RDS_HTTP_ENDPOINT]", x-amzn-errortype ValidationException.
Why: the class is unmodeled so no generated SDK client matches it, and the text is not AWS's, so a client that distinguishes an unsupported source type from a generic input error cannot.
Fix: the enum validation on the CreateDataSource handler's input path — map to the modeled BadRequestException and emit AWS's sentence.
Traces: appsync-create-data-source-relational-source-type-enum#task#2 (cluster also covers appsync-data-source-type-enum-refusal#task#3/#task#4 and appsync-http-data-source-authorization-type-enum#task#4/#task#5).

=== Cause 2 of 10: CreateFunction runtime.name ===
AWS: Smithy (appsync) CreateFunction, runtime: AppSyncRuntime whose name is enum RuntimeName (APPSYNC_JS); modeled BadRequestException|400. Recorded behavior for a value outside the enum is 400 with the fixed message 'The provided runtime is not supported.'
Emulator: "1 validation error detected: Value at 'runtime.name' failed to satisfy constraint: Member must satisfy enum value set: [APPSYNC_JS]".
Why: same as Cause 1 — the documented unsupported-runtime diagnosis is replaced by generic constraint text.
Fix: the runtime validation in the CreateFunction handler (shared with CreateResolver/UpdateFunction).
Traces: appsync-function-runtime-name-enum-rejection#task#3.

=== Cause 3 of 10: CreateFunction syncConfig.conflictHandler ===
AWS: Smithy (appsync) CreateFunction, SyncConfig$conflictHandler: enum ConflictHandlerType (OPTIMISTIC_CONCURRENCY, LAMBDA, AUTOMERGE, NONE); modeled BadRequestException|400. Recorded behavior for a value outside the enum is 400 with the fixed message 'Conflict Handler specified must be a valid Conflict Handler type.'
Emulator: "1 validation error detected: Value at 'syncConfig.conflictHandler' failed to satisfy constraint: Member must satisfy enum value set: [OPTIMISTIC_CONCURRENCY, LAMBDA, AUTOMERGE, NONE]", x-amzn-errortype ValidationException.
Why: as Cause 1.
Fix: the SyncConfig validation shared by the CreateFunction/UpdateFunction/CreateResolver handlers.
Traces: appsync-function-sync-config-conflict-handler-enum-rejection#task#3.

=== Cause 4 of 10: ListGraphqlApis apiType ===
AWS: Smithy (appsync) ListGraphqlApis, apiType: enum GraphQLApiType (GRAPHQL, MERGED) bound as a query filter; modeled BadRequestException|400. Recorded behavior for a value outside the enum is 400 with the fixed message 'Invalid api type filter input.'
Emulator: "1 validation error detected: Value at 'apiType' failed to satisfy constraint: Member must satisfy enum value set: [GRAPHQL, MERGED]", x-amzn-errortype ValidationException.
Why: as Cause 1.
Fix: the query-parameter enum validation in the ListGraphqlApis handler.
Traces: appsync-list-graphql-apis-filter-enum-rejections#task#2 (cluster also covers #task#3 and #task#4).

=== Cause 5 of 10: CreateResolver metricsConfig ===
AWS: Smithy (appsync) CreateResolver, metricsConfig: enum ResolverLevelMetricsConfig (ENABLED, DISABLED); modeled BadRequestException|400. Recorded behavior for a value outside the enum is 400 with the fixed message 'Invalid metricsConfig for resolver given.'
Emulator: "1 validation error detected: Value at 'metricsConfig' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]", x-amzn-errortype ValidationException.
Why: as Cause 1; note AWS words the resolver and data-source variants of this member differently (Cause 7), so one shared generic message cannot satisfy either.
Fix: the metricsConfig validation in the CreateResolver handler.
Traces: appsync-resolver-metrics-config-enum-rejection#task#4.

=== Cause 6 of 10: AssociateSourceGraphqlApi sourceApiAssociationConfig.mergeType ===
AWS: Smithy (appsync) AssociateSourceGraphqlApi, SourceApiAssociationConfig$mergeType: enum MergeType (MANUAL_MERGE, AUTO_MERGE); modeled BadRequestException|400. Recorded behavior for a value outside the enum is 400 with the fixed message 'Invalid MergeType value'.
Emulator: "1 validation error detected: Value at 'sourceApiAssociationConfig.mergeType' failed to satisfy constraint: Member must satisfy enum value set: [MANUAL_MERGE, AUTO_MERGE]", x-amzn-errortype ValidationException.
Why: as Cause 1.
Fix: the SourceApiAssociationConfig validation in the AssociateSourceGraphqlApi handler.
Traces: appsync-source-association-merge-type-enum#task#7 (cluster also covers #task#8).

=== Cause 7 of 10: UpdateSourceApiAssociation sourceApiAssociationConfig.mergeType ===
AWS: same member on UpdateSourceApiAssociation; modeled BadRequestException|400; recorded behavior is 400 'Invalid MergeType value'.
Emulator: the same generic enum frame and ValidationException as Cause 6, on the update path.
Why: as Cause 1; it is a separate handler and is replayed separately.
Fix: the SourceApiAssociationConfig validation in the UpdateSourceApiAssociation handler.
Traces: appsync-update-association-merge-type-outside-enum#task#9 (cluster also covers #task#10 and #task#11).

=== Cause 8 of 10: UpdateDataSource metricsConfig ===
AWS: Smithy (appsync) UpdateDataSource, metricsConfig: enum DataSourceLevelMetricsConfig (ENABLED, DISABLED); modeled BadRequestException|400. Recorded behavior for a value outside the enum is 400 with the fixed message 'Invalid metricsConfig for data source given.' — note 'data source', distinct from the resolver wording in Cause 5.
Emulator: "1 validation error detected: Value at 'metricsConfig' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]", x-amzn-errortype ValidationException.
Why: as Cause 1.
Fix: the metricsConfig validation in the UpdateDataSource handler.
Traces: appsync-update-data-source-metrics-config-enum#task#3 (cluster also covers #task#4 and appsync-update-data-source-type-outside-enum#task#3/#task#4).

=== Cause 9 of 10: UpdateGraphqlApi enhancedMetricsConfig.dataSourceLevelMetricsBehavior ===
AWS: Smithy (appsync) UpdateGraphqlApi, EnhancedMetricsConfig$dataSourceLevelMetricsBehavior: enum (FULL_REQUEST_DATA_SOURCE_METRICS, PER_DATA_SOURCE_METRICS); modeled BadRequestException|400. Recorded behavior for a value outside the enum is 400 with the fixed message 'Invalid dataSourceLevelMetricsBehavior given.' (the sibling resolverLevelMetricsBehavior member has its own analogous sentence).
Emulator: "1 validation error detected: Value at 'enhancedMetricsConfig.dataSourceLevelMetricsBehavior' failed to satisfy constraint: Member must satisfy enum value set: [FULL_REQUEST_DATA_SOURCE_METRICS, PER_DATA_SOURCE_METRICS]", x-amzn-errortype ValidationException.
Why: as Cause 1.
Fix: the EnhancedMetricsConfig validation in the UpdateGraphqlApi handler.
Traces: appsync-update-graphql-api-data-source-metrics-enum#task#2 (cluster also covers #task#3 and appsync-update-graphql-api-resolver-metrics-behavior-enum#task#2/#task#3).

=== Cause 10 of 10: UpdateResolver kind ===
AWS: Smithy (appsync) UpdateResolver, kind: enum ResolverKind (UNIT, PIPELINE); modeled BadRequestException|400. Recorded behavior for a value outside the enum is 400 with the fixed message 'Invalid Resolver kind.'
Emulator: "1 validation error detected: Value at 'kind' failed to satisfy constraint: Member must satisfy enum value set: [UNIT, PIPELINE]", x-amzn-errortype ValidationException.
Why: as Cause 1.
Fix: the kind validation in the UpdateResolver handler (shared with CreateResolver).
Traces: appsync-update-resolver-kind-outside-enum#task#5 (cluster also covers #task#6 and appsync-update-resolver-metrics-config-enum#task#5/#task#6).

## Evidence
Every cause: status 400 on both sides; $.message diverges and, where the recording asserts it, $headers.x-amzn-errortype comes back 'ValidationException' — a shape the appsync Smithy model does not define (smithy_query shape=ValidationException -> "shape 'ValidationException' not found", did_you_mean BadRequestException/NotFoundException).
Cause 1 — expected 'Invalid relational database source type', got "1 validation error detected: Value at 'relationalDatabaseConfig.relationalDatabaseSourceType' failed to satisfy constraint: Member must satisfy enum value set: [RDS_HTTP_ENDPOINT]".
Cause 2 — expected 'The provided runtime is not supported.', got "...Value at 'runtime.name' ... enum value set: [APPSYNC_JS]".
Cause 3 — expected 'Conflict Handler specified must be a valid Conflict Handler type.', got "...Value at 'syncConfig.conflictHandler' ... enum value set: [OPTIMISTIC_CONCURRENCY, LAMBDA, AUTOMERGE, NONE]".
Cause 4 — expected 'Invalid api type filter input.', got "...Value at 'apiType' ... enum value set: [GRAPHQL, MERGED]".
Cause 5 — expected 'Invalid metricsConfig for resolver given.', got "...Value at 'metricsConfig' ... enum value set: [ENABLED, DISABLED]".
Cause 6/7 — expected 'Invalid MergeType value', got "...Value at 'sourceApiAssociationConfig.mergeType' ... enum value set: [MANUAL_MERGE, AUTO_MERGE]".
Cause 8 — expected 'Invalid metricsConfig for data source given.', got the same generic [ENABLED, DISABLED] frame as Cause 5.
Cause 9 — expected 'Invalid dataSourceLevelMetricsBehavior given.', got "...Value at 'enhancedMetricsConfig.dataSourceLevelMetricsBehavior' ... enum value set: [FULL_REQUEST_DATA_SOURCE_METRICS, PER_DATA_SOURCE_METRICS]".
Cause 10 — expected 'Invalid Resolver kind.', got "...Value at 'kind' ... enum value set: [UNIT, PIPELINE]".

## Steps to Reproduce
For each cause, send the operation with the named member set to a value outside its modeled enum (e.g. CreateDataSource(relationalDatabaseConfig={'relationalDatabaseSourceType':'NOT_A_TYPE'}), CreateFunction(runtime={'name':'NOT_JS','runtimeVersion':'1.0.0'}), CreateFunction(syncConfig={'conflictHandler':'NOT_A_HANDLER'}), ListGraphqlApis(apiType='NOT_A_TYPE'), CreateResolver(metricsConfig='NOT_A_VALUE'), AssociateSourceGraphqlApi / UpdateSourceApiAssociation(sourceApiAssociationConfig={'mergeType':'NOT_A_MERGE'}), UpdateDataSource(metricsConfig='NOT_A_VALUE'), UpdateGraphqlApi(enhancedMetricsConfig={'dataSourceLevelMetricsBehavior':'NOT_A_BEHAVIOR',...}), UpdateResolver(kind='NOT_A_KIND')). Each must answer 400 with x-amzn-errortype naming the modeled BadRequestException and the operation's own refusal sentence quoted above — never the generic 'N validation error(s) detected' enum frame and never ValidationException.
