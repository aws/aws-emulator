Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ssm.parameter-tag-dropped-from-template-is-removed-by-update
Issue-Key: cloudformation__ssm.parameter-tag-dropped-from-template-is-removed-by-update|run#15|AssertionError|assert after == {"sim2real": "true"}, (
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# cloudformation__ssm: a tag dropped from an AWS::SSM::Parameter template is not removed by the stack update

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: the SSM parameter resource handler does not remove tags that the updated template no longer declares.
AWS: on a stack update, a resource's tags are reconciled to the template — a tag present in the previous template and absent from the new one is removed from the resource. After an update that drops 'team', ListTagsForResource on the parameter returns only the remaining tags (here the stack-level 'sim2real' tag).
Emulator: after the update the parameter still carries the dropped tag — {'sim2real': 'true', 'team': 'blue'} where only {'sim2real': 'true'} should remain.
Why: tags accumulate and never shrink, so cost allocation, tag-based IAM conditions and tag-scoped automation keep matching resources the template no longer tags.
Fix: the AWS::SSM::Parameter resource handler's update path in the CloudFormation crate — compute the tag delta and call RemoveTagsFromResource for dropped keys.
Traces: cloudformation__ssm.parameter-tag-dropped-from-template-is-removed-by-update.

## Evidence
"tags after the update dropped 'team': {'sim2real': 'true', 'team': 'blue'}, expected only {'sim2real': 'true'}".

## Steps to Reproduce
Deploy a stack with an AWS::SSM::Parameter carrying Tags [{team: blue}], then update the stack with that tag removed from the template; read the parameter's tags with ListTagsForResource.
