Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__logs.sam-autopublishalias-referenceable-alias-and-version cloudformation__lambda__logs.sam-cloudwatchlogs-event-creates-a-subscription-filter
Issue-Key: cloudformation__lambda__logs|CreateStack|ValidationError
Issue-Key: cloudformation__lambda__logs.sam-cloudwatchlogs-event-creates-a-subscription-filter|run#10|AssertionError|assert "AWS::Logs::SubscriptionFilter" in types.values(), f"no subscription filter in {types!r}"
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# cloudformation SAM transform: AutoPublishAlias creates no <Fn>.Alias/<Fn>.Version resources (template refused as an unresolved dependency), and a CloudWatchLogs event creates no subscription filter

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 2 — `AutoPublishAlias` does not synthesise the alias and version resources, so a template referencing them is refused outright.
AWS: `AWS::Serverless::Function` with `AutoPublishAlias: <name>` expands to an `AWS::Lambda::Version` logical id `<FunctionLogicalId>Version<hash>` and an `AWS::Lambda::Alias` logical id `<FunctionLogicalId>Alias<name>`, and `<FunctionLogicalId>.Alias` / `<FunctionLogicalId>.Version` are referenceable from Outputs and from other resources. Recorded behavior only (a SAM transform rule).
Emulator: CreateStack refuses the whole template with `{'Type': 'Sender', 'Code': 'ValidationError', 'Message': 'Template format error: Unresolved resource dependencies [Fn.Alias] in the Outputs block of the template'}` — the transform never created the alias, so the reference cannot resolve. Fixed text is `Template format error: Unresolved resource dependencies [` ... `] in the Outputs block of the template`; `Fn.Alias` is the logical reference from the template.
Traces: cloudformation__lambda__logs.sam-autopublishalias-referenceable-alias-and-version.

Cause 2 of 2 — a SAM `CloudWatchLogs` event source creates no subscription filter.
AWS: `Events: { <Name>: { Type: CloudWatchLogs, Properties: { LogGroupName, FilterPattern } } }` expands to an `AWS::Logs::SubscriptionFilter` on the named log group targeting the function, plus the `AWS::Lambda::Permission` that lets logs invoke it. Recorded behavior only.
Emulator: the stack's resources are only `{'Fn': 'AWS::Lambda::Function', 'FnRole': 'AWS::IAM::Role', 'Source': 'AWS::Logs::LogGroup'}` — no subscription filter resource exists and no filter is registered on the log group.
Traces: cloudformation__lambda__logs.sam-cloudwatchlogs-event-creates-a-subscription-filter.

Why: cause 1 makes every SAM application that uses AutoPublishAlias (the canonical safe-deployment pattern) undeployable — the failure is a template-format error, so nothing in the stack is created. Cause 2 deploys a stack that looks complete while the function is never wired to the log group, so log-driven invocations silently never happen.

Fix: the SAM transform — AutoPublishAlias must emit the Version/Alias resources and register `<Fn>.Alias`/`<Fn>.Version` as resolvable references before template-dependency resolution runs; the CloudWatchLogs event source must emit AWS::Logs::SubscriptionFilter and the matching AWS::Lambda::Permission.

## Evidence
Cause 1: "CreateStack refused the template: {'Type': 'Sender', 'Code': 'ValidationError', 'Message': 'Template format error: Unresolved resource dependencies [Fn.Alias] in the Outputs block of the template'}".
Cause 2: "no subscription filter in {'Fn': 'AWS::Lambda::Function', 'FnRole': 'AWS::IAM::Role', 'Source': 'AWS::Logs::LogGroup'}".

## Steps to Reproduce
1. Deploy a SAM template with AWS::Serverless::Function, AutoPublishAlias: live, and an Output referencing Fn.Alias — CreateStack returns ValidationError 'Unresolved resource dependencies [Fn.Alias]'.
2. Deploy a SAM template whose function has an Events entry of Type: CloudWatchLogs over an AWS::Logs::LogGroup in the same stack; list stack resources and DescribeSubscriptionFilters on the group — no subscription filter exists.
