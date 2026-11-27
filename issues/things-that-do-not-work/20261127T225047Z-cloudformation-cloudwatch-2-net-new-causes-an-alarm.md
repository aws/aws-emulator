Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch.cfn-alarm-tag-update-applied-in-place cloudformation__cloudwatch.cfn-metric-stream-getatt-state-and-dates
Issue-Key: cloudformation__cloudwatch.cfn-alarm-tag-update-applied-in-place|run#17|AssertionError|assert user_tags() == {"team": "checkout"}, (
Issue-Key: cloudformation__cloudwatch.cfn-metric-stream-getatt-state-and-dates|run#14|AssertionError|assert outputs["State"] == live["State"] == "running", (
Issue-Commit: 37bf0949a3a203e7d1b7ebeb80108d58d67e6afd

# cloudformation + cloudwatch: 2 net-new causes — an alarm tag update merges instead of replacing the tag set, and GetAtt State on AWS::CloudWatch::MetricStream is null

> _Note: this issue was identified against the code base at commit `37bf0949a3a203e7d1b7ebeb80108d58d67e6afd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Two net-new causes on CloudFormation-provisioned CloudWatch resources.

Cause 1 of 2: an in-place stack update that rewrites the Tags of an AWS::CloudWatch::Alarm leaves the removed tags on the live alarm.
AWS: the template's Tags list is the complete desired tag set for the resource. An update whose template carries exactly one tag leaves the alarm carrying exactly that tag: CloudFormation untags the keys the new template no longer declares (ListTagsForResource on the alarm then returns only the declared tag plus CloudFormation's own aws:cloudformation:* system tags). Recorded behaviour only for the ordering of the tag/untag calls.
Emulator: the new tag is added and the dropped tags are kept, so the alarm reads back the union of the old and new tag sets.
Why: tags drive cost allocation and IAM tag-condition policies. A caller that removes a tag through a stack update believes the tag is gone while the live alarm still carries it, so a policy keyed on that tag still grants access.
Fix: the AWS::CloudWatch::Alarm resource handler's update path — it must diff the declared tag set against the live one and issue UntagResource for the keys the template dropped (never untagging aws:cloudformation:* system tags).

Cause 2 of 2: Fn::GetAtt on AWS::CloudWatch::MetricStream for the State attribute resolves to null.
AWS: the resource exposes read-only attributes (State, CreationDate, LastUpdateDate) that are filled from the stream itself; GetMetricStream on a provisioned stream reports State `running`, and the stack output taken from Fn::GetAtt <logical-id>.State carries that same value. The two reads must agree.
Emulator: GetMetricStream answers `running` while Fn::GetAtt State is absent/null, so the stack output has no value.
Why: a template that wires a stream's State into an Output, a Condition or another resource's property gets an empty value, and any consumer that reads the output sees nothing where AWS gives `running`.
Fix: the AWS::CloudWatch::MetricStream resource handler's read/attribute projection — State, CreationDate and LastUpdateDate must be populated from the stream record that backs GetMetricStream.

Traces: cloudformation__cloudwatch.cfn-alarm-tag-update-applied-in-place; cloudformation__cloudwatch.cfn-metric-stream-getatt-state-and-dates.

## Evidence
Cause 1 — after an update whose template declares only team=checkout:
  expected the alarm to carry only team=checkout
  actual:  {'team': 'payments', 'tier': 'gold'}

Cause 2 — provisioned AWS::CloudWatch::MetricStream:
  'GetAtt State is None, GetMetricStream says running; both must be running'

## Steps to Reproduce
Cause 1:
1. CreateStack with an AWS::CloudWatch::Alarm whose Tags are [{team: payments}, {tier: gold}].
2. UpdateStack with the same alarm but Tags [{team: checkout}].
3. ListTagsForResource on the alarm ARN — expect only team=checkout (plus aws:cloudformation:* system tags); the emulator returns team=payments and tier=gold.

Cause 2:
1. CreateStack with an AWS::CloudWatch::MetricStream and an Output whose Value is Fn::GetAtt <logical-id>.State.
2. DescribeStacks and read the output; call GetMetricStream for the same stream. Expect both to read 'running'; the emulator's output value is absent.
