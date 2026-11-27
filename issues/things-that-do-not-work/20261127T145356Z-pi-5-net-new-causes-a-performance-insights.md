Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pi.get-resource-metrics-answers-every-query-whatever-the-token pi.list-available-resource-dimensions-metric-other-than-db-load-rejected pi.list-available-resource-metrics-answers-one-page-whatever-the-token pi.instance-metric-and-dimension-vocabulary-rejected pi.list-available-resource-dimensions-unknown-metric-rejected pi.list-available-resource-dimensions-docdb-service-type-for-rds-instance-rejected pi.list-performance-analysis-reports-unissued-next-token-rejected
Issue-Shape-Id: com.amazonaws.pi#PerformanceInsightsv20180227
Issue-Key: pi.get-resource-metrics-answers-every-query-whatever-the-token|_source_instance#1|AssertionError|raise AssertionError(f"the source instance was still {state!r} at its deadline")
Issue-Key: pi.list-available-resource-dimensions-metric-other-than-db-load-rejected|_source_instance#1|AssertionError|raise AssertionError(f"the source instance was still {state!r} at its deadline")
Issue-Key: pi.list-available-resource-metrics-answers-one-page-whatever-the-token|_source_instance#2|AssertionError|raise AssertionError(f"the source instance was still {state!r} at its deadline")
Issue-Key: pi.instance-metric-and-dimension-vocabulary-rejected|run#11|AssertionError|assert got == ("InvalidArgumentException", f"osx type is unknown. Valid values are: {valid_types}"), (
Issue-Key: pi.list-available-resource-dimensions-unknown-metric-rejected|run#10|AssertionError|assert got == ("InvalidArgumentException", _ONLY_DB_LOAD), (
Issue-Key: pi.list-available-resource-dimensions-docdb-service-type-for-rds-instance-rejected|_refusal#10|AssertionError|raise AssertionError(f"expected a refusal, got a success: {resp.get('MetricDimensions')!r}")
Issue-Key: pi.list-performance-analysis-reports-unissued-next-token-rejected|run#10|AssertionError|assert (code, message) == ("InvalidArgumentException", _EXPECTED_MESSAGE), (
Issue-Commit: ae4983b2ee9be0c2b746a2748bc3435fe07ec59f

# pi: 5 net-new causes — a Performance-Insights source DB instance never leaves 'creating', ListAvailableResourceDimensions ignores the data source's engine, and three InvalidArgumentException messages are emulator-authored

> _Note: this issue was identified against the code base at commit `ae4983b2ee9be0c2b746a2748bc3435fe07ec59f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAvailableResourceDimensions, ListAvailableResourceMetrics, ListPerformanceAnalysisReports, GetResourceMetrics

## Description
Five distinct root causes, all grounded in failing checks the behavioral suite lists under `### pi`. They are bundled because they share one service token; a fixer must close all five before submitting.

None of these is covered by the open issue 20261120T153000Z-pi-rds-the-published-db-instance-endpoint-is-unreachable-from-the-replay-client.md: that issue is about dialing a published endpoint, whereas Cause 1 below is a control-plane status that never advances and Causes 2-5 are request-validation/refusal-wording defects that need no DB session at all.

---
Cause 1 of 5: a Performance-Insights source DB instance never leaves DBInstanceStatus 'creating'.

AWS: recorded behavior only (lifecycle, not modeled as an error). rds CreateDBInstance stores the instance with DBInstance$DBInstanceStatus 'creating' and the service advances it to 'available'; DescribeDBInstances then reports 'available' and DbiResourceId/PerformanceInsightsEnabled can be read off it. The recordings for the sibling pi checks in this same suite show an instance reaching 'available' well inside a 1200 s wait, so provisioning completing is the recorded behavior.
Emulator: for the three checks below, DescribeDBInstances keeps answering DBInstanceStatus 'creating' for the whole wait budget (the checks wait min(1500 s, time_left-600 s) with backoff) and then abort with "the source instance was still 'creating' at its deadline". The instance never reaches 'available', 'failed' or any other terminal status.
Hypothesis: the only members that distinguish the three stuck create requests from the two sibling pi checks whose instances DID reach 'available' are StorageType "gp2" (the two that settle use "gp3") and PerformanceInsightsRetentionPeriod 7 (one settling sibling sends 465, the other sends none). Neither value is refused at create time — the request is accepted with 200 — so whatever gate holds the record back is silent.
Why: every client that polls for 'available' before using an instance (the documented and SDK-waiter behavior) hangs forever instead of proceeding, and no Performance Insights read against an RDS source can be exercised at all: three pi data-plane/pagination behaviors are unreachable through this emulator.
Fix: the rds CreateDBInstance handler's provisioning transition and the engine state's status settle — the pending-status progression that should carry 'creating' to 'available' (see the citations under Traces).
Traces: pi.get-resource-metrics-answers-every-query-whatever-the-token, pi.list-available-resource-dimensions-metric-other-than-db-load-rejected, pi.list-available-resource-metrics-answers-one-page-whatever-the-token. Code: services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs:907 (settle_due_transitions), :2384-2459 (PendingStatus / provisioning_to).

---
Cause 2 of 5: ListAvailableResourceMetrics refuses an unknown MetricTypes member with an emulator-authored sentence.

AWS: Smithy: pi ListAvailableResourceMetrics.MetricTypes (MetricTypeList) @required; modeled errors InvalidArgumentException (HTTP 400), NotAuthorizedException, InternalServiceError. The member is a free-form string list, so the vocabulary check is the service's. The recorded refusal is HTTP 400 InvalidArgumentException with the message "<type> type is unknown. Valid values are: os, db, db.sql.stats, db.sql_tokenized.stats" — fixed text: " type is unknown. Valid values are: " followed by the published metric-type list; variable part: the offending MetricTypes member exactly as the request sent it, unquoted, leading the sentence. One unknown member refuses the whole request: MetricTypes=["db", "osx"] is refused with that same message naming osx, and the valid member does not rescue it.
Emulator: HTTP 400 InvalidArgumentException with 'MetricTypes must name only metric types Performance Insights publishes (os, db, db.sql.stats, db.sql_tokenized.stats); got "osx".' — a different sentence, and it quotes the offending value instead of leading with it bare.
Why: a client that matches on the refusal text (or surfaces it to an operator) sees a message AWS never emits; the code and status are right, the wording is not.
Fix: the ListAvailableResourceMetrics handler's MetricTypes validation message.
Traces: pi.instance-metric-and-dimension-vocabulary-rejected (the check stops at this assertion, so its later GetDimensionKeyDetails group/dimension/key-id assertions were never reached and are not part of this cause). Code: services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/list_available_resource_metrics.rs:233-237.

---
Cause 3 of 5: ListAvailableResourceDimensions refuses a non-db.load metric with the wrong message, and accepts db.sampledload.

AWS: Smithy: pi ListAvailableResourceDimensions.Metrics (DimensionsMetricList, @length 1..5) @required, documented "The types of metrics for which to retrieve dimensions. Valid values include db.load."; modeled error InvalidArgumentException (HTTP 400). The recorded refusal is HTTP 400 InvalidArgumentException with the message 'The specified isn\'t supported. ListAvailableResourceDimensions supports only the db.load metric. Set Metrics to ["db.load"], and try again.' — that whole sentence is fixed text (AWS's own wording leaves the offending-value slot empty after "The specified"), with no variable part. It applies both to a single unsupported metric (Metrics=["db.load.avg"]) and to a list that pairs db.load with another metric (Metrics=["db.load", "os.cpuUtilization.user.avg"]), which is refused whole.
Emulator: HTTP 400 InvalidArgumentException with 'Metrics must name metric types this service publishes dimensions for (db.load, db.sampledload); got db.load.avg.' — a different sentence, and it admits db.sampledload as a second supported metric, which AWS's "supports only the db.load metric" rule refuses.
Why: the refusal text does not match AWS, and the accepted vocabulary is wider than AWS's, so a request AWS refuses is served a dimension catalog here.
Fix: the ListAvailableResourceDimensions handler's Metrics validation — both the accepted set and the refusal message.
Traces: pi.list-available-resource-dimensions-unknown-metric-rejected. Code: services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/list_available_resource_dimensions.rs:206-218 and the DIMENSIONED_METRIC_TYPES constant at :322.

---
Cause 4 of 5: ListAvailableResourceDimensions never checks ServiceType against the engine behind Identifier, so DOCDB on an RDS instance is served instead of refused.

AWS: Smithy: pi ListAvailableResourceDimensions.ServiceType (enum DOCDB | RDS) @required, Identifier documented as the data source's DbiResourceId; modeled error InvalidArgumentException (HTTP 400). The recorded refusal for a DOCDB ServiceType naming an RDS instance's DbiResourceId is HTTP 400 InvalidArgumentException with "The specified ServiceType is invalid for engine <engine>." — fixed text "The specified ServiceType is invalid for engine " and a trailing ".", with the addressed instance's engine as the only variable part (taken from the instance the Identifier names, e.g. postgres). The same Identifier under ServiceType RDS is served, and is still served after the refusal.
Emulator: HTTP 200. The request is answered with a DocumentDB-only dimension catalog for the RDS instance's DbiResourceId — db.load with groups db, db.application, db.blocking_object, db.blocking_session, db.blocking_sql, db.host, db.query, db.query_tokenized, db.wait_state. The handler filters the catalog by the requested ServiceType alone and never resolves the engine of the data source (its own doc comment states that "the engine behind a DbiResourceId is not part of it").
Why: a client gets an authoritative-looking DocumentDB dimension vocabulary for a PostgreSQL/MySQL instance, so a mis-typed ServiceType is never caught and the breakdown dimensions a caller then queries by do not exist on that engine.
Fix: the ListAvailableResourceDimensions handler must resolve the engine of the data source the Identifier names and refuse a ServiceType that does not own it, before filtering the catalog.
Traces: pi.list-available-resource-dimensions-docdb-service-type-for-rds-instance-rejected. Code: services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/list_available_resource_dimensions.rs:225-233 and :324-365.

---
Cause 5 of 5: ListPerformanceAnalysisReports refuses an unissued NextToken with an emulator-authored sentence.

AWS: Smithy: pi ListPerformanceAnalysisReports.NextToken (@length 1..8192, @pattern ^[a-zA-Z0-9_=-]+$); modeled error InvalidArgumentException (HTTP 400). A token that matches the pattern but was never handed out by the service is refused HTTP 400 InvalidArgumentException with the message "Specify a valid token." — the entire sentence is fixed text; it names neither the token, the ServiceType nor the Identifier, so it has no variable part.
Emulator: HTTP 400 InvalidArgumentException with "The NextToken was not issued for this request's ServiceType and Identifier."
Why: the status and code are right but the message is not AWS's, so a client matching the refusal text (or logging it) sees wording AWS never emits.
Fix: the ListPerformanceAnalysisReports handler's page-token decode failure message.
Traces: pi.list-performance-analysis-reports-unissued-next-token-rejected. Code: services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/list_performance_analysis_reports.rs:226-239 (message at :231).

## Evidence
Cause 1 — expected: DescribeDBInstances eventually reports DBInstanceStatus 'available' for the created Performance-Insights source instance. Actual: AssertionError: the source instance was still 'creating' at its deadline (three checks, each after its full provisioning budget of up to 1500 s).

Cause 2 — expected: ('InvalidArgumentException', 'osx type is unknown. Valid values are: os, db, db.sql.stats, db.sql_tokenized.stats'). Actual: ('InvalidArgumentException', 'MetricTypes must name only metric types Performance Insights publishes (os, db, db.sql.stats, db.sql_tokenized.stats); got "osx".').

Cause 3 — expected: ('InvalidArgumentException', 'The specified isn\'t supported. ListAvailableResourceDimensions supports only the db.load metric. Set Metrics to ["db.load"], and try again.'). Actual: ('InvalidArgumentException', 'Metrics must name metric types this service publishes dimensions for (db.load, db.sampledload); got db.load.avg.').

Cause 4 — expected: ('InvalidArgumentException', 'The specified ServiceType is invalid for engine postgres.') for ListAvailableResourceDimensions(ServiceType='DOCDB', Identifier=<RDS postgres DbiResourceId>, Metrics=['db.load']). Actual: HTTP 200 with MetricDimensions = [{'Metric': 'db.load', 'Groups': [{'Group': 'db', ...}, {'Group': 'db.application', ...}, {'Group': 'db.blocking_object', ...}, {'Group': 'db.blocking_session', ...}, {'Group': 'db.blocking_sql', ...}, {'Group': 'db.host', ...}, {'Group': 'db.query', ...}, {'Group': 'db.query_tokenized', ...}, {'Group': 'db.wait_state', ...}]}] — i.e. the DocumentDB-only groups served for an RDS instance.

Cause 5 — expected: InvalidArgumentException 'Specify a valid token.'. Actual: InvalidArgumentException "The NextToken was not issued for this request's ServiceType and Identifier.".

## Steps to Reproduce
Cause 1: rds.create_db_subnet_group over the VPC's subnets, then rds.create_db_instance(Engine='mysql', DBInstanceClass='db.t3.medium', AllocatedStorage=20, StorageType='gp2', BackupRetentionPeriod=0, MultiAZ=False, PubliclyAccessible=False, EnablePerformanceInsights=True, PerformanceInsightsRetentionPeriod=7, DBSubnetGroupName=..., VpcSecurityGroupIds=[...]); poll rds.describe_db_instances(DBInstanceIdentifier=...)['DBInstances'][0]['DBInstanceStatus'] with backoff — it stays 'creating' indefinitely. Compare with the same create using StorageType='gp3' and no PerformanceInsightsRetentionPeriod, which does reach 'available'.

Cause 2: with an available PI-enabled mysql instance's DbiResourceId, call pi.list_available_resource_metrics(ServiceType='RDS', Identifier=dbi, MetricTypes=['osx']) and again with MetricTypes=['db','osx']; inspect Error.Code/Error.Message.

Cause 3: with an available PI-enabled postgres instance's DbiResourceId, call pi.list_available_resource_dimensions(ServiceType='RDS', Identifier=dbi, Metrics=['db.load.avg']) and pi.list_available_resource_dimensions(ServiceType='RDS', Identifier=dbi, Metrics=['db.load','os.cpuUtilization.user.avg']); inspect Error.Message. Also call it with Metrics=['db.sampledload'] — it is served instead of refused.

Cause 4: with that same postgres instance's DbiResourceId, call pi.list_available_resource_dimensions(ServiceType='DOCDB', Identifier=dbi, Metrics=['db.load']) — it answers 200 instead of refusing; then call it again with ServiceType='RDS' to confirm the RDS answer.

Cause 5: call pi.list_performance_analysis_reports(ServiceType='RDS', Identifier=dbi, NextToken='c2ltMnJlYWwtbmV2ZXItaXNzdWVkLXRva2Vu') — a token matching ^[a-zA-Z0-9_=-]+$ that the service never issued — and inspect Error.Message.
