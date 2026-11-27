Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events__sqs.cfn-events-rule-target-nested-required-members-rejected
Issue-Key: cloudformation__events__sqs.cfn-events-rule-target-nested-required-members-rejected|run#17|AssertionError|assert found == want, f"{label}: validation errors expected {want!r}, got {found!r}"
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# CloudFormation: template validation does not report required properties missing inside a nested target property (Targets[0].EcsParameters.TaskDefinitionArn)

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: nested required-member validation is not performed.
AWS: CloudFormation validates required properties inside nested resource property structures. A template whose `AWS::Events::Rule` declares `Targets[0].EcsParameters` without `TaskDefinitionArn` (Smithy: EcsParameters.TaskDefinitionArn is a required member of the EventBridge Target.EcsParameters structure) produces a resource validation failure whose status reason is `Required property [TaskDefinitionArn] not found` reported against the JSON pointer `/Resources/<LogicalId>/Properties/Targets/0/EcsParameters`, and the stack does not reach CREATE_COMPLETE. Fixed text: `Required property [<MemberName>] not found`; the variable parts are the member name and the property pointer taken from the submitted template.
Emulator: produces no validation errors at all for that template — the recorded list of validation errors is empty.
Why: a client deploying an incomplete ECS target gets no diagnostic naming the missing member or its location, and the rule is accepted with a target EventBridge can never invoke.
Fix: the CloudFormation template/resource property validator (it must recurse into nested property structures and emit one validation error per missing required member, carrying the member name and the property pointer).
Traces: cloudformation__events__sqs.cfn-events-rule-target-nested-required-members-rejected.

## Evidence
`AssertionError: EcsParameters missing TaskDefinitionArn: validation errors expected [('FAILED', 'FAIL', 'Required property [TaskDefinitionArn] not found', '/Resources/Rule/Properties/Targets/0/EcsParameters')], got []`

## Steps to Reproduce
1. CreateStack with an `AWS::Events::Rule` whose `Targets` entry has `EcsParameters` present but without `TaskDefinitionArn`.
2. Collect the stack's resource/validation failures; the emulator reports none.
3. Replay cloudformation__events__sqs.cfn-events-rule-target-nested-required-members-rejected.
