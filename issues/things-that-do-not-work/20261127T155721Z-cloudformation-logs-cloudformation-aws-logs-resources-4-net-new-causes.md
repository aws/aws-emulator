Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__logs.cfn-log-group-tags-update-replaces-the-live-tags cloudformation__logs.cfn-query-definition-is-created-and-ref-returns-its-id cloudformation__logs.deploy-logs-query-definition-tags-property-rejected cloudformation__logs.deploy-logs-query-definition-template-naming-an-existing-id-rejected cloudformation__logs.deploy-logs-log-stream-group-change-creates-the-stream-anew-and-keeps-the-old-one
Issue-Key: cloudformation__logs.cfn-log-group-tags-update-replaces-the-live-tags|run#12|AssertionError|assert own() == {"env": "beta"}, f"after the Tags update the live tags are {own()!r}; expected only env=beta"
Issue-Key: cloudformation__logs.cfn-query-definition-is-created-and-ref-returns-its-id|run#8|AssertionError|assert len(defs) == 1, f"DescribeQueryDefinitions found {defs!r} for the stack's definition {name!r}"
Issue-Key: cloudformation__logs.deploy-logs-query-definition-tags-property-rejected|_until_count#10|Timeout|time.sleep(2)
Issue-Key: cloudformation__logs.deploy-logs-query-definition-template-naming-an-existing-id-rejected|_wait_for_one#10|Timeout|time.sleep(2)
Issue-Key: cloudformation__logs.deploy-logs-log-stream-group-change-creates-the-stream-anew-and-keeps-the-old-one|run#31|AssertionError|assert seen == [stream], f"home group streams after the move: {len(seen)}, ours {stream in seen}"
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# cloudformation + AWS::Logs resources: 4 net-new causes — a LogGroup Tags update keeps the old tags, QueryDefinition is never registered with CloudWatch Logs, invalid QueryDefinition templates never settle, and a LogStream's LogGroupName change creates no stream

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 4 — an `AWS::Logs::LogGroup` `Tags` update does not replace the live tags.
AWS: updating the `Tags` property of a log group replaces the resource-managed tag set, so after an update from `team=alpha` to `env=beta` ListTagsForResource on the group returns only `env=beta`. Recorded behavior only (the CloudFormation registry schema makes Tags an updatable property).
Emulator: after the update the live tags are still the pre-update set — `{'team': 'alpha'}` — so the update was accepted but not applied.
Traces: cloudformation__logs.cfn-log-group-tags-update-replaces-the-live-tags.

Cause 2 of 4 — `AWS::Logs::QueryDefinition` creates a stack resource that CloudWatch Logs does not know about.
AWS: the resource creates a real query definition; `Ref` returns its `queryDefinitionId` and `DescribeQueryDefinitions` lists it by `queryDefinitionName`. Smithy (logs): `DescribeQueryDefinitions` returns `queryDefinitions[].queryDefinitionId`/`name`.
Emulator: the stack reaches CREATE_COMPLETE but `DescribeQueryDefinitions` returns `[]` for the stack's definition name.
Traces: cloudformation__logs.cfn-query-definition-is-created-and-ref-returns-its-id.

Cause 3 of 4 — a QueryDefinition template that must be refused never settles.
AWS: a template declaring an unsupported `Tags` property on `AWS::Logs::QueryDefinition`, and one naming a `QueryDefinitionId` that already exists, both fail the resource and the stack settles in `ROLLBACK_COMPLETE` with the handler's failure reason. Recorded behavior only.
Emulator: neither stack reaches a terminal status within the run — both checks end `Check timed out after 60.0s`. Hypothesis: the QueryDefinition resource provider neither succeeds nor reports a handler failure, so the stack operation never completes.
Traces: cloudformation__logs.deploy-logs-query-definition-tags-property-rejected, cloudformation__logs.deploy-logs-query-definition-template-naming-an-existing-id-rejected.

Cause 4 of 4 — changing an `AWS::Logs::LogStream` `LogGroupName` creates no stream in the new group.
AWS: `LogGroupName` is a create-only property, so changing it replaces the resource: a stream with the same name is created in the new log group and the stream in the old group is left behind (Retain/replacement semantics). Recorded behavior only.
Emulator: after the update the new ('home') group has 0 log streams and the stack's stream is absent from it.
Traces: cloudformation__logs.deploy-logs-log-stream-group-change-creates-the-stream-anew-and-keeps-the-old-one.

Why: cause 1 means tag-driven cost allocation and tag-based IAM conditions keep matching the pre-update tags, with the stack reporting UPDATE_COMPLETE. Cause 2 produces a stack that claims to own a saved query no console or API call can find. Cause 3 hangs any deployment that hits an invalid QueryDefinition template — the client never gets a verdict and cannot roll back. Cause 4 leaves a declared log stream non-existent, so writers targeting it fail at runtime.

Fix: the AWS::Logs::LogGroup provider's update path (tag replacement), the AWS::Logs::QueryDefinition provider (create must call logs:PutQueryDefinition and must report handler failures so the stack can roll back), and the AWS::Logs::LogStream provider's replacement path.

## Evidence
Cause 1: "after the Tags update the live tags are {'team': 'alpha'}; expected only env=beta".
Cause 2: "DescribeQueryDefinitions found [] for the stack's definition 'sim2real-behav-cfnqd-a4e91fff03eb'".
Cause 3: both checks end with "Error: Check timed out after 60.0s"; expected ROLLBACK_COMPLETE with the handler's failure reason.
Cause 4: "home group streams after the move: 0, ours False".

## Steps to Reproduce
1. Deploy an AWS::Logs::LogGroup with Tags team=alpha, update the template to Tags env=beta, then ListTagsForResource on the group — still team=alpha.
2. Deploy an AWS::Logs::QueryDefinition and call DescribeQueryDefinitions — empty.
3. Deploy an AWS::Logs::QueryDefinition template carrying a Tags property, and one naming an existing QueryDefinitionId — the stack never reaches a terminal status.
4. Deploy an AWS::Logs::LogStream, change its LogGroupName to a second group, and DescribeLogStreams on the new group — no stream.
