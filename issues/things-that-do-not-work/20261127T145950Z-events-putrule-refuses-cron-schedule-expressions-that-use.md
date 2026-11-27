Issue-Source: trace-sweep-divergence
Issue-Trace: events-put-rule-cron-last-nearest-weekday-and-nth-weekday-wildcards-accepted
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-put-rule-cron-last-nearest-weekday-and-nth-weekday-wildcards-accepted events-put-rule-cron-last-weekday-of-month-wildcard events-put-rule-cron-overnight-hour-wraparound
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|PutRule|2xx->4xx|$.RuleArn
Issue-Key: events|PutRule|2xx->4xx|status
Issue-Key: events|DescribeRule|2xx->4xx|$.Arn
Issue-Key: events|DescribeRule|2xx->4xx|$.CreatedBy
Issue-Key: events|DescribeRule|2xx->4xx|$.EventBusName
Issue-Key: events|DescribeRule|2xx->4xx|$.Name
Issue-Key: events|DescribeRule|2xx->4xx|$.ScheduleExpression
Issue-Key: events|DescribeRule|2xx->4xx|$.State
Issue-Key: events|DescribeRule|2xx->4xx|$.__type
Issue-Key: events|DescribeRule|2xx->4xx|$.message
Issue-Key: events|DescribeRule|2xx->4xx|status
Issue-Commit: f9b7321d6a22920f7cbed43b33d059dedecbc1ee

# events: PutRule refuses cron schedule expressions that use the L, W and # wildcards

> _Note: this issue was identified against the code base at commit `f9b7321d6a22920f7cbed43b33d059dedecbc1ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRule

## Description
2 replayed divergence clusters, 1 root cause (with its knock-on read).

Cause 1 of 1: the schedule-expression parser rejects valid EventBridge cron wildcards.
AWS: accepts a six-field cron(...) expression that uses the L (last day/last weekday of month), W (nearest weekday) and # (nth weekday of month) wildcards, as well as a wrap-around hour range, and answers HTTP 200 with RuleArn; a following DescribeRule returns the rule with ScheduleExpression echoed verbatim (e.g. 'cron(0 12 L * ? *)'), State 'DISABLED', CreatedBy=<account id>, EventBusName 'default' and the rule's Arn (Smithy: PutRule.ScheduleExpression -> ScheduleExpression @length 0..256, documented as 'The scheduling expression. For example, "cron(0 20 * * ? *)" or "rate(5 minutes)"'; the model constrains only the length, so the accepted cron grammar is recorded behavior - these wildcards are part of EventBridge's cron syntax).
Emulator: answers HTTP 400 and returns no RuleArn, so the rule is never created; the fixture's following DescribeRule then answers 400 ResourceNotFoundException 'Rule <name> does not exist on EventBus default.' where AWS returns the rule. Five steps across three fixtures fail this way (the L-of-month, nearest-weekday and nth-weekday wildcards and the overnight hour wrap-around).
Why: these wildcards are the normal way to express 'last business day of the month', 'nearest weekday to the 15th' and 'third Friday' schedules, so every such rule is impossible to create against the emulator - a client's infrastructure deployment fails outright on a schedule real EventBridge accepts, and the 400 carries no indication that the expression is in fact supported upstream.
Fix: the cron parser behind PutRule's ScheduleExpression validation in the events rule handler (day-of-month/day-of-week field support for L, W and #, and hour ranges that wrap past midnight); DescribeRule's 404 is only the consequence of the rule never being stored.
Traces: events-put-rule-cron-last-nearest-weekday-and-nth-weekday-wildcards-accepted#task#1 (PutRule; cluster also covers #task#3, #task#5, events-put-rule-cron-last-weekday-of-month-wildcard#task#1 and events-put-rule-cron-overnight-hour-wraparound#task#1), #task#2 (DescribeRule; cluster also covers #task#4, #task#6 and the #task#2 steps of the other two fixtures).

## Evidence
PutRule (events-put-rule-cron-last-nearest-weekday-and-nth-weekday-wildcards-accepted#task#1): "status: expected 200, got 400"; "$.RuleArn: missing (expected 'arn:aws:events:us-west-2:000000000000:rule/<rule>', got None)" - 5 affected steps.
DescribeRule (#task#2): "status: expected 200, got 400"; "$.Name: missing (expected '<rule>', got None)"; "$.ScheduleExpression: missing (expected 'cron(0 12 L * ? *)', got None)"; "$.State: missing (expected 'DISABLED', got None)"; "$.EventBusName: missing (expected 'default', got None)"; "$.CreatedBy: missing (expected '000000000000', got None)"; "$.__type: extra (expected None, got 'ResourceNotFoundException')"; "$.message: extra (expected None, got 'Rule <rule> does not exist on EventBus default.')".

## Steps to Reproduce
1. PutRule(Name=<rule>, ScheduleExpression='cron(0 12 L * ? *)', State='DISABLED') - expect 200 with RuleArn, observe 400.
2. Repeat with 'cron(0 12 15W * ? *)' (nearest weekday), 'cron(0 12 ? * 6#3 *)' (nth weekday) and an overnight hour range such as 'cron(0 22-2 * * ? *)'.
3. DescribeRule(<rule>) after each - expect the rule with ScheduleExpression echoed, observe ResourceNotFoundException.
4. Re-replay events-put-rule-cron-last-nearest-weekday-and-nth-weekday-wildcards-accepted#task#1 and #task#2.
