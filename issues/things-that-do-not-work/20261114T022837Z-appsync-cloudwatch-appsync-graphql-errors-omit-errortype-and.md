Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cloudwatch.obs-appsync-1-resolver-datasource-metrics-opt-in-matrix appsync__cloudwatch.obs-appsync-3-4xx-error-alarm-closed-loop appsync__cloudwatch.obs-appsync-3-unauthorized-burst-on-one-api-does-not-breach-sibling-api-alarm appsync__cloudwatch.appsync-enhanced-metrics-publish-under-api-id-resolver-dimensions
Issue-Key: appsync__cloudwatch.obs-appsync-1-resolver-datasource-metrics-opt-in-matrix|run#25|AssertionError|assert _failed(warm_fail), (
Issue-Key: appsync__cloudwatch.obs-appsync-3-4xx-error-alarm-closed-loop|_poll_until#34|Timeout|time.sleep(interval_s)
Issue-Key: appsync__cloudwatch.obs-appsync-3-unauthorized-burst-on-one-api-does-not-breach-sibling-api-alarm|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: appsync__cloudwatch.appsync-enhanced-metrics-publish-under-api-id-resolver-dimensions|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: 98ddcdb392adb33f11d22fc873f87d432bd2bd05

# appsync__cloudwatch: AppSync GraphQL errors omit errorType, and no AWS/AppSync metric series (4XXError, enhanced per-Resolver/DataSource/Operation) is ever published (3 causes)

> _Note: this issue was identified against the code base at commit `98ddcdb392adb33f11d22fc873f87d432bd2bd05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Three net-new root causes behind 4 failing behavioral checks under the appsync__cloudwatch heading. A fixer must close ALL THREE before submitting.

Note on an existing open issue: 20261110T162302Z-appsync-cloudwatch-appsync-publishes-real-aws-hostnames-in-graphqlapi-uris.md lists these same check ids, but its replay keys are all bound to a PublishedUrlRejected egress refusal on uris.GRAPHQL. In this replay the published GraphQL endpoint IS reachable — obs-appsync-1 received HTTP 200 GraphQL bodies from the emulator's resolver — so that issue does not cover what fails now. These are different causes behind the same checks, and none of the four checks was reported as owned by any open issue.

Cause 1 of 3: a GraphQL error raised by util.error(message, errorType) carries no errorType member.
AWS: the AppSync GraphQL data plane answers HTTP 200 with a GraphQL response envelope whose `errors` list entries carry, besides `message` and `path`, an `errorType` member holding exactly the second argument of util.error(message, errorType) (and `data` for the field is null). The error-entry shape is not in the Smithy model (the GraphQL POST is not a modeled operation) — recorded behavior only, as asserted by the check: exactly one error entry, errors[0].errorType == the errorType string passed to util.error, and data.<field> == null. The fixed text of the entry is the `message` argument verbatim; the variable parts (`message`, `errorType`, `path`) all come from the request's resolver code and the selected field.
Emulator: for a resolver whose APPSYNC_JS request handler calls util.error('<message>', '<errorType>'), the response is HTTP 200 {'data': {'c': None}, 'errors': [{'path': ['c'], 'message': '<message>'}]} — the entry has `path` and `message` only; `errorType` is absent entirely (no key, not an empty string).
Why: a client that branches on errorType (the documented way to distinguish an application error class from a transport or auth error, and the way AppSync's own GraphQLError metric is categorised) sees `None` for every error, so it cannot tell an injected/business error from any other failure, and SDK/Amplify error handling that keys on errorType silently takes the default path.
Fix: the AppSync GraphQL execution path's error serializer in the appsync service (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice) — it must propagate the errorType argument of util.error (and util.appendError) into each `errors[]` entry of the GraphQL response envelope.
Traces: appsync__cloudwatch.obs-appsync-1-resolver-datasource-metrics-opt-in-matrix (AssertionError: "Query.c with fail=true must return exactly one GraphQL error of type 'Sim2RealInjected' and a null field, got (200, {'data': {'c': None}, 'errors': [{'path': ['c'], 'message': 'sim2real injected failure'}]})").

Cause 2 of 3: AppSync publishes no AWS/AppSync 4XXError datapoints, so every alarm over that metric is dead.
AWS: GraphQL requests rejected by the API's authorization (e.g. an API_KEY api called with an unknown x-api-key, answered 401) are counted in the CloudWatch namespace AWS/AppSync under MetricName 4XXError, dimensioned solely on GraphQLAPIId = the api's apiId, one Sum per 60-second bucket, visible within about a minute. The namespace/dimension is documented AppSync behavior (https://docs.aws.amazon.com/appsync/latest/devguide/monitoring.html) and the per-api dimension is the single api-level key; the datapoint values are derived from the request count, never from a recording. A CloudWatch metric alarm with Statistic=Sum, Period=60, EvaluationPeriods=1, Threshold=3, GreaterThanOrEqualToThreshold, TreatMissingData=notBreaching must therefore move OK -> ALARM on the bucket containing 4 rejected requests and deliver its AlarmActions notification; the sibling api's series for that same minute must read 0 and its alarm stay OK.
Emulator: no AWS/AppSync 4XXError datapoint ever appears for the api. GetMetricStatistics(Namespace=AWS/AppSync, MetricName=4XXError, Dimensions=[{GraphQLAPIId:<apiId>}], Period=60, Statistics=[Sum]) returns an empty Datapoints list for the whole window, so the polling loop exhausts the check's own 600 s metric deadline and the run reports 'Check timed out after 600.0s'. The alarm consequently never leaves OK and no notification is delivered.
Why: a client cannot observe AppSync client-error rate at all; every 4XXError alarm, dashboard and metric-math expression over it is permanently silent, and the api-isolation guarantee (one api's errors must not move another api's alarm) is untestable because neither api ever has a series.
Fix: AWS/AppSync metric emission for the GraphQL data plane in services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice, writing 4XXError (and the sibling 5XXError/Requests/Latency/GraphQLError transport series) into the cloudwatch metric store (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801) under the GraphQLAPIId dimension.
Traces: appsync__cloudwatch.obs-appsync-3-4xx-error-alarm-closed-loop, appsync__cloudwatch.obs-appsync-3-unauthorized-burst-on-one-api-does-not-breach-sibling-api-alarm (both: Error: Check timed out after 600.0s).

Cause 3 of 3: enhanced metrics are accepted as configuration but never emitted, so no per-Resolver / per-DataSource / per-Operation series exists.
AWS: Smithy com.amazonaws.appsync#EnhancedMetricsConfig (members resolverLevelMetricsBehavior, dataSourceLevelMetricsBehavior, operationLevelMetricsConfig, all required) documents that, when enabled, AppSync emits granular data to CloudWatch: resolver metrics (GraphQL errors, Requests, Latency, cache hits/misses) "recorded by API ID and resolver name", data-source metrics (Requests, Latency, Errors) "recorded by API ID and data source name", and operation metrics (Requests, GraphQL errors) "recorded by API ID and operation name". With PER_RESOLVER_METRICS / PER_DATA_SOURCE_METRICS, only the resolvers and data sources whose metricsConfig is ENABLED produce a series; FULL_REQUEST_RESOLVER_METRICS later overrides a resolver's opt-out. The series reuse the transport metric names (Requests, Latency, GraphQLError) in AWS/AppSync and add exactly one key beside GraphQLAPIId: Resolver ("<typeName>.<fieldName>"), DataSource (the data source name) or Operation (the root operation type).
Emulator: the enhancedMetricsConfig and the per-resolver/per-data-source metricsConfig values are stored and echoed, but no enhanced series is ever published: the check that drives traffic through an opted-in resolver and polls AWS/AppSync for Requests/Latency/GraphQLError on [GraphQLAPIId, Resolver] / [GraphQLAPIId, DataSource] never observes a datapoint and the run reports 'Check timed out after 60.0s' without ever reaching an assertion on the opted-out side. This is the same emission gap as Cause 2, one level down: the dimension sets that only exist when enhanced metrics are on.
Why: a client that turns enhanced metrics on (and pays for them) gets nothing — no per-resolver latency, no per-data-source error count, no per-operation request count — so the opt-in matrix itself is unobservable and resolver-level alarming is impossible.
Fix: enhanced-metric emission in the appsync resolver/data-source execution path (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice), honouring resolverLevelMetricsBehavior / dataSourceLevelMetricsBehavior / operationLevelMetricsConfig and the per-resource metricsConfig opt-in, and writing the series with the Resolver / DataSource / Operation dimension alongside GraphQLAPIId into the cloudwatch metric store.
Traces: appsync__cloudwatch.appsync-enhanced-metrics-publish-under-api-id-resolver-dimensions (Error: Check timed out after 60.0s).

## Evidence
Cause 1 — expected: HTTP 200 with exactly one errors[] entry whose errorType equals the errorType argument given to util.error (here the resolver calls util.error('<message>', '<errorType>')) and data.<field> == null. Actual: HTTP 200 {'data': {'c': None}, 'errors': [{'path': ['c'], 'message': 'sim2real injected failure'}]} — the errorType member is missing from the error entry. Assertion text from the replay: "Query.c with fail=true must return exactly one GraphQL error of type 'Sim2RealInjected' and a null field, got (200, {'data': {'c': None}, 'errors': [{'path': ['c'], 'message': 'sim2real injected failure'}]})".

Cause 2 — expected: after N GraphQL requests in one 60 s bucket are rejected 401 for a forged API key, GetMetricStatistics(Namespace='AWS/AppSync', MetricName='4XXError', Dimensions=[{'Name':'GraphQLAPIId','Value':<apiId>}], Period=60, Statistics=['Sum']) returns one datapoint with Sum == N, and an alarm with Threshold=3/GreaterThanOrEqualToThreshold/EvaluationPeriods=1/TreatMissingData=notBreaching transitions OK -> ALARM and publishes its AlarmActions notification; the sibling api's datapoint for that bucket is 0 and its alarm stays OK. Actual: Datapoints is empty for the entire window on both apis, the alarms stay OK, and the check is killed by its own metric deadline — 'Error: Check timed out after 600.0s' on both obs-appsync-3 checks.

Cause 3 — expected: with enhancedMetricsConfig={'resolverLevelMetricsBehavior':'PER_RESOLVER_METRICS','dataSourceLevelMetricsBehavior':'PER_DATA_SOURCE_METRICS','operationLevelMetricsConfig':'DISABLED'} and metricsConfig=ENABLED on one resolver/data source, AWS/AppSync Requests/Latency/GraphQLError datapoints appear under [GraphQLAPIId=<apiId>, Resolver='Query.<field>'] and [GraphQLAPIId=<apiId>, DataSource=<dsName>] for the opted-in pair and under no dimension set for the opted-out pair. Actual: the configuration round-trips but no enhanced-dimension datapoint is ever returned; the run reports 'Error: Check timed out after 60.0s'.

## Steps to Reproduce
Cause 1:
1. appsync CreateGraphqlApi (authenticationType=API_KEY), StartSchemaCreation with a schema exposing Query.c(m: String!, fail: Boolean!), CreateDataSource (type=NONE), CreateApiKey.
2. CreateResolver for Query.c, runtime APPSYNC_JS 1.0.0, with request code that calls util.error('<message>', '<errorType>') when ctx.args.fail is true.
3. POST {"query":"query{ c(m:\"x\", fail:true) }"} to uris.GRAPHQL with header x-api-key: <key>.
4. Observe the response: 200 with errors[0] = {path, message} and no errorType member; AWS returns errors[0].errorType == '<errorType>' and data.c == null.

Cause 2:
1. Create an API_KEY GraphQL api with a working Query.ping resolver and an api key; verify a valid request answers 200.
2. cloudwatch PutMetricAlarm: Namespace='AWS/AppSync', MetricName='4XXError', Dimensions=[{GraphQLAPIId:<apiId>}], Statistic=Sum, Period=60, EvaluationPeriods=1, Threshold=3, ComparisonOperator=GreaterThanOrEqualToThreshold, TreatMissingData=notBreaching, AlarmActions=[<sns topic>]; wait for OK.
3. Within one minute, POST the same query 4 times with a forged key 'da2-xxxxxxxxxxxxxxxxxxxxxxxxxx' (each answered 401).
4. Poll cloudwatch GetMetricStatistics for that namespace/metric/dimension -> Datapoints stays empty; DescribeAlarms shows StateValue OK forever and the SQS queue subscribed to the topic receives nothing.
5. Repeat with two apis to see neither api ever gets a series (the isolation assertion is never reached).

Cause 3:
1. appsync CreateGraphqlApi with enhancedMetricsConfig={'resolverLevelMetricsBehavior':'PER_RESOLVER_METRICS','dataSourceLevelMetricsBehavior':'PER_DATA_SOURCE_METRICS','operationLevelMetricsConfig':'DISABLED'}.
2. CreateDataSource twice (type=NONE) with metricsConfig ENABLED and DISABLED; CreateResolver for Query.a (ENABLED, on the ENABLED data source) and Query.b (DISABLED, on the DISABLED data source).
3. Send several successful requests to Query.a and Query.b with a valid api key.
4. Poll cloudwatch GetMetricStatistics in Namespace 'AWS/AppSync' for MetricName Requests/Latency/GraphQLError with Dimensions=[{GraphQLAPIId:<apiId>},{Resolver:'Query.a'}] and with [{GraphQLAPIId:<apiId>},{DataSource:<dsOn>}] -> no datapoints are ever returned, for either the opted-in or the opted-out pair.
