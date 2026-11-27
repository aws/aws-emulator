Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__sqs__ssm.opsitem-notification-topic-receives-create-and-update-events sns__sqs__ssm.opsitem-moved-to-another-topic-stops-notifying-the-first sns__sqs__ssm.opsitem-with-a-malformed-notification-arn-rejected
Issue-Key: sns__sqs__ssm.opsitem-notification-topic-receives-create-and-update-events|_await_event#11|AssertionError|raise AssertionError(
Issue-Key: sns__sqs__ssm.opsitem-moved-to-another-topic-stops-notifying-the-first|_collect_until#9|AssertionError|raise AssertionError(f"{what} not seen in {budget:.0f}s; received {seen!r}")
Issue-Key: sns__sqs__ssm.opsitem-with-a-malformed-notification-arn-rejected|_refusal#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# sns__sqs__ssm: 2 causes — OpsItem notifications are never published to the configured SNS topic, and a malformed Notifications ARN is accepted

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOpsItem

## Description
Cause 1 of 2: an OpsItem's Notifications topic receives nothing.
AWS: an OpsItem carrying Notifications[].Arn publishes a CREATE_OPSITEM notification to that SNS topic on create and an update notification on update; moving the OpsItem to another topic stops notifications to the first and starts them on the second.
Emulator: an SQS queue subscribed to the topic receives nothing at all in 50 s — neither the create nor the update notification, so the topic-move behaviour cannot be observed either.
Why: OpsItem notification wiring is silently dead; on-call paths built on it deliver nothing and nothing in the API says so.
Fix: the OpsItem notification publisher in the SSM crate (ops/create_ops_item.rs / ops/update_ops_item.rs → SNS Publish).
Traces: sns__sqs__ssm.opsitem-notification-topic-receives-create-and-update-events, sns__sqs__ssm.opsitem-moved-to-another-topic-stops-notifying-the-first.

Cause 2 of 2: CreateOpsItem accepts a malformed notification ARN.
AWS: a Notifications[].Arn that is not a well-formed SNS topic ARN is refused (HTTP 400, OpsItemInvalidParameterException naming the member); no OpsItem is created.
Emulator: returns 200 with OpsItemId/OpsItemArn and stores the item with the malformed ARN.
Why: an OpsItem exists whose notification target can never resolve, and the caller is never told.
Fix: validate Notifications[].Arn in ops/create_ops_item.rs.
Traces: sns__sqs__ssm.opsitem-with-a-malformed-notification-arn-rejected.

## Evidence
Cause 1: 'no CREATE_OPSITEM notification for the OpsItem reached the queue in 50s; received []'; 'both CREATE_OPSITEM notifications on the first topic not seen in 50s; received []'.
Cause 2: "expected the call to be refused, but it succeeded: {'OpsItemId': 'oi-2fd8012ffae8', 'OpsItemArn': 'arn:aws:ssm:us-west-2:000000000000:opsitem/oi-2fd8012ffae8', HTTPStatusCode: 200}".

## Steps to Reproduce
1. Create an SNS topic with an SQS subscription; CreateOpsItem with Notifications=[{Arn: <topic>}], then UpdateOpsItem; poll the queue for CREATE_OPSITEM/update notifications. Repeat after moving the OpsItem's notification to a second topic.
2. CreateOpsItem with Notifications=[{Arn: '<not an SNS ARN>'}].
