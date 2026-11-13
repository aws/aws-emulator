Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.op-fifo-attributes-on-standard-queue-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.op-fifo-attributes-on-standard-queue-rejected|run#8|AssertionError|assert err_fifo.get("Code") == "InvalidAttributeName", f"got {err_fifo!r}"
Issue-Commit: 582ed21b383f102b650a2b551c0e79525419c00c

# sqs: SetQueueAttributes naming FifoQueue on a standard queue is refused InvalidAttributeValue ("Modifying queue type is not supported") instead of AWS's InvalidAttributeName "Unknown Attribute FifoQueue."

> _Note: this issue was identified against the code base at commit `582ed21b383f102b650a2b551c0e79525419c00c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetQueueAttributes

## Description
1 net-new root cause, on a check the replay report did NOT mark ALREADY FILED or OWNED. Note the FIXED issue 20261029T180649Z-sqs-queue-attribute-state-and-response-payloads-diverge.md, whose Cause 1 establishes AWS's message template for exactly this error class on CreateQueue ("Unknown Attribute " + attribute name + "." with __type com.amazonaws.sqs#InvalidAttributeName); that issue is about CreateQueue and does not reach SetQueueAttributes, so this is not a reopen of it. This is not a refusal of an operation a fixed issue claimed to fix: SetQueueAttributes refuses the call in both AWS and the emulator — the error CODE and MESSAGE are what diverge.

--- Cause 1 of 1: a FIFO-only attribute is classified as a known-but-immutable value rather than as a name that does not exist for this operation ---
AWS: SetQueueAttributes binds both com.amazonaws.sqs#InvalidAttributeName ("The specified attribute doesn't exist.", HTTP 400 defaulted) and com.amazonaws.sqs#InvalidAttributeValue ("A queue attribute value is invalid.", HTTP 400 defaulted) — Smithy: SetQueueAttributes errors {InvalidAddress 404, InvalidAttributeName 400, InvalidAttributeValue 400, InvalidSecurity 403, OverLimit 403, QueueDoesNotExist 400 (awsQueryError AWS.SimpleQueueService.NonExistentQueue), RequestThrottled 403, UnsupportedOperation 400}. For the FIFO-only attributes, AWS answers the NAME error: a SetQueueAttributes on a standard queue naming FifoQueue is refused HTTP 400, error code InvalidAttributeName, Type Sender, message "Unknown Attribute FifoQueue." — fixed text "Unknown Attribute " + the attribute name as the request spelled it + "." (message wording is recorded behavior only; the error shape and status are modeled). The same rule is applied to ContentBasedDeduplication on a standard queue ("Unknown Attribute ContentBasedDeduplication."), and the emulator already gets THAT one right in the same request sequence, so the divergence is specific to FifoQueue.
Emulator: answers HTTP 400 with Code 'InvalidAttributeValue' (QueryErrorCode 'InvalidAttributeValue', Type 'Sender') and the emulator-authored message "Invalid value for the parameter FifoQueue. Reason: Modifying queue type is not supported." The choice is stated in the handler's own module doc, services/service-com-amazonaws-sqs-amazonsqs/src/ops/set_queue_attributes.rs lines 18-24: "That attribute is one the service HAS, so a call naming it is refused for the change it asks for (`InvalidAttributeValue`, [`crate::state::unmodifiable_attribute_value`]) rather than as a name that does not exist." The classification is produced by state::validate_attributes(&input.attributes, false, fifo) returning AttributeRefusal::Value for the create_only attribute, which the handler maps to InvalidAttributeName/InvalidAttributeValue at lines 248-257.
Why: callers branch on the error code, not the sentence. Code that catches InvalidAttributeName to mean "this attribute is not settable here" (the idiomatic guard, and the one AWS raises for every FIFO-only attribute on a standard queue) does not catch the emulator's InvalidAttributeValue and the exception escapes; conversely a handler for InvalidAttributeValue (a bad VALUE, normally retried with a corrected value) is entered for a request whose value was never the problem. Message-asserting tests and error-mapping tables keyed on AWS's "Unknown Attribute <name>." template also fail.
Fix: the FIFO-only/create-only branch of SetQueueAttributes' attribute validation — state::validate_attributes in the SQS crate's state (the create_only path that currently calls state::unmodifiable_attribute_value), as consumed by services/service-com-amazonaws-sqs-amazonsqs/src/ops/set_queue_attributes.rs: classify an attribute that is not settable on this operation/queue type as AttributeRefusal::Name, carrying the message "Unknown Attribute " + the requested attribute name + ".", so the refusal surfaces as InvalidAttributeName (HTTP 400, Type Sender) — the same shape the handler already produces for ContentBasedDeduplication. The queue's stored type must stay unchanged, as it already does.
Traces: sqs.op-fifo-attributes-on-standard-queue-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator; per-service pass rate sqs 259/264 (98%).

[mismatch] sqs.op-fifo-attributes-on-standard-queue-rejected
  File check_sqs_op_fifo_attributes_on_standard_queue_rejected.py, line 42, in run
    assert err_fifo.get("Code") == "InvalidAttributeName", f"got {err_fifo!r}"
  AssertionError: got {'Message': 'Invalid value for the parameter FifoQueue. Reason: Modifying queue type is not supported.', 'Code': 'InvalidAttributeValue', 'QueryErrorCode': 'InvalidAttributeValue', 'Type': 'Sender'}

Request sequence (all against ONE standard queue created by CreateQueue with no attributes):
1. SetQueueAttributes(Attributes={'ContentBasedDeduplication': 'true'})
   expected: Code 'InvalidAttributeName', Message 'Unknown Attribute ContentBasedDeduplication.'
   actual:   PASSED — the emulator already answers exactly this.
2. SetQueueAttributes(Attributes={'FifoQueue': 'true'})
   expected: Code 'InvalidAttributeName', Message 'Unknown Attribute FifoQueue.'
   actual:   Code 'InvalidAttributeValue', Message 'Invalid value for the parameter FifoQueue. Reason: Modifying queue type is not supported.'
In the expected message, "Unknown Attribute " and the trailing "." are the fixed text and 'FifoQueue' is the attribute name taken from the request's Attributes map. In the emulator's message, "Invalid value for the parameter " / "Reason: Modifying queue type is not supported." is emulator-authored wording AWS does not produce for this call.

The check never reached its later legs, which the emulator is not shown to fail: GetQueueAttributes(All) must not contain a FifoQueue key (the standard queue must not have been mutated), and the positive control SetQueueAttributes(VisibilityTimeout='45') followed by a read-back of '45' must still succeed on the same queue.

Source confirmation: services/service-com-amazonaws-sqs-amazonsqs/src/ops/set_queue_attributes.rs module doc (lines 18-24) states the create_only attribute is deliberately refused as InvalidAttributeValue via crate::state::unmodifiable_attribute_value, and the handler's map at lines 248-257 turns state::AttributeRefusal::Value into SetQueueAttributesErrorModeled::InvalidAttributeValue.

## Steps to Reproduce
Boot the emulator and replay sqs.op-fifo-attributes-on-standard-queue-rejected from suite simparity-behavioral-aws-v1 (report heading sqs):
1. CreateQueue with a plain name and no Attributes (a STANDARD queue).
2. SetQueueAttributes(QueueUrl=<that url>, Attributes={'ContentBasedDeduplication': 'true'}) — expect HTTP 400, Code 'InvalidAttributeName', Type 'Sender', Message 'Unknown Attribute ContentBasedDeduplication.' (the emulator already matches).
3. SetQueueAttributes(QueueUrl=<that url>, Attributes={'FifoQueue': 'true'}) — expect HTTP 400, Code 'InvalidAttributeName', Type 'Sender', Message 'Unknown Attribute FifoQueue.'. Against the emulator this comes back as Code 'InvalidAttributeValue' with Message 'Invalid value for the parameter FifoQueue. Reason: Modifying queue type is not supported.'
4. GetQueueAttributes(AttributeNames=['All']) — the map must still carry no 'FifoQueue' key.
5. Positive control: SetQueueAttributes(Attributes={'VisibilityTimeout': '45'}) must succeed and read back as '45' on the same queue.
