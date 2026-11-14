Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__opensearch.e2e-osdocstat-document-request-status-logs-lifecycle logs__opensearch.osdocstat-create-domain-probes-each-granted-log-group
Issue-Key: logs__opensearch.e2e-osdocstat-document-request-status-logs-lifecycle|phase#4|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: logs__opensearch.osdocstat-create-domain-probes-each-granted-log-group|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Commit: eb654e46cb0eaddecbed5c077f0424b114c18ecc

# logs__opensearch: 2 net-new causes — a CFN stack whose OpenSearch domain is granted log delivery by an account-scoped AWS::Logs::ResourcePolicy rolls back, and an accepted CreateDomain never writes the two create-time probe log streams into each granted log group

> _Note: this issue was identified against the code base at commit `eb654e46cb0eaddecbed5c077f0424b114c18ecc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Two net-new root causes, both on the OpenSearch log-publishing path, both grounded in failing behavioral checks under the suite heading `### logs__opensearch`. Neither is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 2: CreateStack of a template that creates four AWS::Logs::LogGroup resources, one account-scoped AWS::Logs::ResourcePolicy granting es.amazonaws.com logs:CreateLogStream + logs:PutLogEvents on those four group ARNs, and one AWS::OpenSearchService::Domain (FGAC, EngineVersion OpenSearch_2.11, single t3.small.search node, 10 GiB gp3, EncryptionAtRest + NodeToNode + EnforceHTTPS, all four LogPublishingOptions enabled, each to its own group, DependsOn the resource policy) reaches ROLLBACK_COMPLETE instead of CREATE_COMPLETE.
AWS: Smithy (opensearch): CreateDomain POST /2021-01-01/opensearch/domain, input member LogPublishingOptions: map<LogType, LogPublishingOption>; LogType enum = INDEX_SLOW_LOGS | SEARCH_SLOW_LOGS | ES_APPLICATION_LOGS | AUDIT_LOGS; LogPublishingOption { CloudWatchLogsLogGroupArn @length 20..2048, Enabled: boolean }; modeled refusal for a bad request is ValidationException (httpError 400). AWS documents (opensearch-service developer guide, createdomain-configure-slow-logs) that the target group must carry a CloudWatch Logs resource policy letting es.amazonaws.com write to it; that grant may be an ACCOUNT-scoped policy (logs:PutResourcePolicy with a policyName, which is exactly what AWS::Logs::ResourcePolicy creates) naming the group ARNs in its Resource list — it does not have to be a resource-scoped policy attached to each group. With that grant in place CloudFormation creates all six resources and the stack reaches CREATE_COMPLETE, exposing the domain's Ref/DomainEndpoint/Arn and the four group names as Outputs.
Emulator: the stack reaches terminal state ROLLBACK_COMPLETE, so the check's phase 1 (deploy) fails before anything else is exercised and the domain, its endpoint and the four group outputs never exist. Hypothesis: the emulator's CreateDomain grant check (the same check that correctly refuses an ungranted group with ValidationException — proven by the other failing check in this issue, which passes that assertion) only recognises a RESOURCE-scoped CloudWatch Logs policy and does not consider an ACCOUNT-scoped one whose Resource list names the group, so the AWS::OpenSearchService::Domain resource is refused ValidationException and CloudFormation rolls the stack back; an unimplemented or non-registering AWS::Logs::ResourcePolicy provider (so the grant is never created at all) produces the same rollback and is the alternative to check first in the stack events.
Why: a client that provisions an OpenSearch domain with log publishing through CloudFormation — the normal way this feature is deployed, and the only way to express the grant and the domain in one stack — gets a rolled-back stack and no domain at all, instead of a working domain. Every downstream use (reading the endpoint output, auditing, slow logs) is unreachable.
Fix: the OpenSearch CreateDomain handler's LogPublishingOptions grant validation (it must accept an account-scoped CloudWatch Logs resource policy whose Resource list covers the named group, not only a resource-scoped one), and the CloudFormation resource provider for AWS::Logs::ResourcePolicy that must create that account-scoped policy before the DependsOn'd domain is created.
Traces: logs__opensearch.e2e-osdocstat-document-request-status-logs-lifecycle (phase 1/5 deploy).

Cause 2 of 2: an accepted CreateDomain carrying LogPublishingOptions never writes the two create-time probe log streams into each granted log group.
AWS: recorded behavior only (the Smithy model states the shape of LogPublishingOptions but nothing about delivery). The recording shows that when CreateDomain is accepted with a log type enabled on a group that grants es.amazonaws.com, the service immediately — before the domain has a node — writes exactly two log streams into each granted group, named `cust-test-log-stream` and `es-test-log-stream`, each holding exactly one event whose message is the fixed text `Amazon OpenSearch Service created a test log event`. Both stream names and that message are fixed text, with no variable part. On the refused side (a group with no grant, CreateDomain refused ValidationException) the recording shows only `cust-test-log-stream` ever lands in that group and never `es-test-log-stream`, because the missing grant blocks the service-principal write.
Emulator: the check exhausts the suite's 60 s cap while polling DescribeLogStreams/GetLogEvents on the two granted groups for those two streams and their event ('Error: Check timed out after 60.0s'), so it reaches no verdict; the control-plane CreateDomain itself is accepted and the ungranted CreateDomain is correctly refused ValidationException. No probe stream or probe event ever appears in any group.
Why: a client that enables log publishing and verifies the wiring the documented way — looking for the service's test log event in the group right after CreateDomain — concludes delivery is broken and cannot distinguish a correctly granted group from an ungranted one, which is the whole point of the probe.
Fix: the OpenSearch CreateDomain handler's log-publishing setup path, which must call CloudWatch Logs CreateLogStream + PutLogEvents once per granted, enabled LogType group (both probe streams on the accepted path; only the caller-side stream reachable on the refused path). Open issue 20261111T000041Z-logs-opensearch-a-domain-with-log-publishing-enabled.md covers DELIVERY of slow-log/application-log events during domain operation and open issue 20261102T090835Z-logs-opensearch-opensearch-createdomain-never-validates-logpublishingoptions-the-legacy-es.md covers VALIDATION of LogPublishingOptions; neither covers the create-time probe streams, which are written by CreateDomain itself before the domain is Active.
Traces: logs__opensearch.osdocstat-create-domain-probes-each-granted-log-group.

## Evidence
Cause 1 — check logs__opensearch.e2e-osdocstat-document-request-status-logs-lifecycle:
  AssertionError: [phase 1/5 deploy] stack sim2real-osds-e2e-<id> reached 'ROLLBACK_COMPLETE', not CREATE_COMPLETE -- read its stack events
  Expected: DescribeStacks StackStatus = CREATE_COMPLETE with Outputs OpenSearchDomainName (= the Ref'd domain), OpenSearchDomainEndpoint, OpenSearchDomainArn and four distinct log-group names (AuditLogGroupName, AppLogGroupName, SlowIndexLogGroupName, SlowSearchLogGroupName).
  Actual: StackStatus = ROLLBACK_COMPLETE; no outputs, no domain.
  Template resources: 4x AWS::Logs::LogGroup (RetentionInDays 3653), 1x AWS::Logs::ResourcePolicy (PolicyName parameter, PolicyDocument allowing Service es.amazonaws.com logs:PutLogEvents + logs:CreateLogStream on the four group ARNs), 1x AWS::OpenSearchService::Domain (DependsOn the resource policy) with all four LogPublishingOptions enabled to those groups.

Cause 2 — check logs__opensearch.osdocstat-create-domain-probes-each-granted-log-group:
  Error: Check timed out after 60.0s (no assertion reached).
  The check first proves the negative side passes: CreateDomain naming an ungranted group is refused with error code ValidationException. It then issues an accepted CreateDomain with ES_APPLICATION_LOGS and SEARCH_SLOW_LOGS to two granted groups and polls each group.
  Expected per group: DescribeLogStreams lists exactly ['cust-test-log-stream', 'es-test-log-stream'] and GetLogEvents on each returns exactly one event with message 'Amazon OpenSearch Service created a test log event'. Expected for the ungranted group: exactly ['cust-test-log-stream'] and never 'es-test-log-stream'.
  Actual: the groups stay empty, so the 180 s poll never satisfies its predicate and the check burns the 60 s harness cap.

## Steps to Reproduce
Cause 1:
1. CreateStack with a template containing four AWS::Logs::LogGroup resources, one AWS::Logs::ResourcePolicy whose PolicyDocument allows Principal Service es.amazonaws.com the actions logs:PutLogEvents and logs:CreateLogStream on the four group ARNs, and one AWS::OpenSearchService::Domain (EngineVersion OpenSearch_2.11, ClusterConfig t3.small.search x1, EBSOptions gp3/10, EncryptionAtRestOptions.Enabled true, NodeToNodeEncryptionOptions.Enabled true, DomainEndpointOptions.EnforceHTTPS true, AdvancedSecurityOptions with an IAM MasterUserARN, AccessPolicies allowing the account root es:*, and LogPublishingOptions AUDIT_LOGS / ES_APPLICATION_LOGS / INDEX_SLOW_LOGS / SEARCH_SLOW_LOGS each Enabled true to one of the four groups), with DependsOn from the domain to the resource policy.
2. Poll DescribeStacks for a terminal status.
3. AWS: CREATE_COMPLETE with all seven Outputs. Emulator: ROLLBACK_COMPLETE. Read DescribeStackEvents to see which resource was refused (expected: the Domain refused ValidationException over the log-group grant, or the AWS::Logs::ResourcePolicy itself failing).

Cause 2:
1. CreateLogGroup for two groups and, for each, PutResourcePolicy granting es.amazonaws.com logs:PutLogEvents + logs:CreateLogStream on '<group-arn>:*' scoped to the domain ARN; create a third group with no policy.
2. CreateDomain (OpenSearch_2.11, t3.small.search x1, gp3/10) with LogPublishingOptions ES_APPLICATION_LOGS -> the ungranted group: it is refused ValidationException (emulator already matches).
3. CreateDomain with ES_APPLICATION_LOGS and SEARCH_SLOW_LOGS enabled to the two granted groups: accepted.
4. Poll DescribeLogStreams + GetLogEvents on each granted group. AWS: both 'cust-test-log-stream' and 'es-test-log-stream' appear, each with one event 'Amazon OpenSearch Service created a test log event'. Emulator: the groups stay empty.
5. Poll the ungranted group. AWS: only 'cust-test-log-stream'. Emulator: empty.
