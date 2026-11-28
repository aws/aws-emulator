Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda.cfn-lambda-stack-tag-update-retags-function-and-leaves-permission-untouched
Issue-Key: cloudformation__lambda.cfn-lambda-stack-tag-update-retags-function-and-leaves-permission-untouched|run#18|AssertionError|assert "Target" in touched and "Grant" not in touched, (
Issue-Commit: 35f7de95e926ecd075d4f0c5efe73729faf5e0c1

# cloudformation: a tag-only stack update emits resource events for resources it did not touch, so an untouched AWS::Lambda::Permission is reported as updated

> _Note: this issue was identified against the code base at commit `35f7de95e926ecd075d4f0c5efe73729faf5e0c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: a stack update that changes only the stack's tags produces resource events for every resource in the template instead of only the ones whose tags are propagated.
AWS: stack-level tags are propagated to resources that support tagging, so a tag-only UpdateStack produces UPDATE events for the taggable resources (here the AWS::Lambda::Function, logical id `Target`) and the stack itself, and no events for resources that carry no tags (here the AWS::Lambda::Permission, logical id `Grant`, and the AWS::IAM::Role) — the untouched resources are left alone. Recorded behavior: the event stream for the tag-only update names `Target` and the stack, and does not name `Grant`.
Emulator: emits events for `['Assume', 'Grant', 'Target', '<stack name>']` — every resource in the template, including the permission statement that CloudFormation does not touch.
Why: a client that watches DescribeStackEvents to learn what a change actually affected is told its resource policy was modified when it was not; blue/green and drift tooling that keys on 'which logical ids moved' takes remediation action on untouched resources, and an operator reading the event stream cannot see the real blast radius of a tag change.
Fix: the update planner in the CloudFormation service — a tag-only stack update must restrict its resource actions (and therefore its emitted events) to resources whose properties or tags actually change, rather than walking the whole template.
Traces: cloudformation__lambda.cfn-lambda-stack-tag-update-retags-function-and-leaves-permission-untouched.

## Evidence
`AssertionError: logical ids with events during the tag-only update: ['Assume', 'Grant', 'Target', 'sim2real-cfptag-stack-<id>']; expected the function 'Target' among them and the permission 'Grant' absent`.

## Steps to Reproduce
Deploy a stack containing an AWS::IAM::Role, an AWS::Lambda::Function and an AWS::Lambda::Permission; then `aws cloudformation update-stack` with the identical template body and only `--tags` changed. Read DescribeStackEvents for the update and note that the permission's logical id has events. Replay check cloudformation__lambda.cfn-lambda-stack-tag-update-retags-function-and-leaves-permission-untouched.
