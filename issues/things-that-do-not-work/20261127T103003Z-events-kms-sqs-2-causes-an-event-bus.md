Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__kms__sqs.create-event-bus-by-key-alias-rejects-unknown-alias-and-delivers-with-known-one events__kms__sqs.update-event-bus-to-missing-key-alias-rejected-and-bus-keeps-its-key events__kms__sqs.bus-moved-from-customer-key-to-aws-owned-key-keeps-routing-by-rule-pattern
Issue-Key: events__kms__sqs.create-event-bus-by-key-alias-rejects-unknown-alias-and-delivers-with-known-one|_refusal#10|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: events__kms__sqs.update-event-bus-to-missing-key-alias-rejected-and-bus-keeps-its-key|_refused#11|AssertionError|raise AssertionError(f"the call was accepted where a refusal was expected: {answer!r}")
Issue-Key: events__kms__sqs.bus-moved-from-customer-key-to-aws-owned-key-keeps-routing-by-rule-pattern|run#25|Timeout|time.sleep(2)
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# events__kms__sqs: 2 causes — an event bus accepts a KmsKeyIdentifier alias no alias holds, and an empty KmsKeyIdentifier never moves the bus back to the AWS owned key

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventBus

## Description
Two distinct root causes, each grounded in failing behavioral checks.

Cause 1 of 2: a KmsKeyIdentifier naming an alias that does not exist is accepted on create and on update.
  AWS: Smithy: CreateEventBus.KmsKeyIdentifier may be 'the key ARN, KeyId, key alias, or key alias ARN', so EventBridge resolves it through KMS; an alias no alias holds cannot resolve and the call is refused (the alias-not-found class, 400), leaving no bus on create and the bus's current key untouched on update. A known alias is then accepted and the bus delivers.
  Emulator: both calls return 200 and echo the unresolvable alias as the bus's KmsKeyIdentifier — CreateEventBus answers {'EventBusArn': ..., 'KmsKeyIdentifier': 'arn:aws:kms:...:alias/<name>'} and UpdateEventBus answers the same with the bus re-keyed to it.
  Why: a typo in an alias is stored as a bus's encryption key instead of being refused; the bus reports a key identifier that resolves to nothing, and the refused-create/unchanged-update invariants do not hold.
  Fix: the eventbridge CreateEventBus and UpdateEventBus handlers' KmsKeyIdentifier resolution — resolve aliases through KMS and refuse an unresolvable one before writing bus state.
  Traces: events__kms__sqs.create-event-bus-by-key-alias-rejects-unknown-alias-and-delivers-with-known-one, events__kms__sqs.update-event-bus-to-missing-key-alias-rejected-and-bus-keeps-its-key.

Cause 2 of 2: UpdateEventBus with an empty KmsKeyIdentifier does not move the bus to the AWS owned key.
  AWS: Smithy: 'If you do not specify a customer managed key identifier, EventBridge uses an Amazon Web Services owned key to encrypt events on the event bus.' The recording shows UpdateEventBus(Name=<bus>, KmsKeyIdentifier='') clearing the bus's customer key: DescribeEventBus stops reporting KmsKeyIdentifier within seconds, and the bus's rule keeps delivering matching events and dropping non-matching ones.
  Emulator: the check waits for DescribeEventBus to stop reporting a key and is cut off at the suite's 60 s cap with no verdict. Hypothesis: the empty KmsKeyIdentifier is ignored (or stored as the empty string) so the bus never stops reporting a key and the wait cannot settle — the same empty-string handling gap filed for kms__lambda's DurableConfig.KMSKeyArn.
  Why: a client moving a bus off a customer managed key (for example to re-enable schema discovery) is told nothing and the bus stays keyed; it can then never retire the old key.
  Fix: the eventbridge UpdateEventBus handler's KmsKeyIdentifier update path — treat an empty identifier as 'clear the customer key' and stop projecting KmsKeyIdentifier from DescribeEventBus.
  Traces: events__kms__sqs.bus-moved-from-customer-key-to-aws-owned-key-keeps-routing-by-rule-pattern.

## Evidence
events__kms__sqs.create-event-bus-by-key-alias-rejects-unknown-alias-and-delivers-with-known-one — AssertionError: expected the call to be refused, but it succeeded: {'EventBusArn': ..., 'KmsKeyIdentifier': 'arn:aws:kms:us-west-2:<account>:alias/sim2real-xsvc-buskey-<id>', 'HTTPStatusCode': 200}.

events__kms__sqs.update-event-bus-to-missing-key-alias-rejected-and-bus-keeps-its-key — AssertionError: the call was accepted where a refusal was expected: {'Arn': ..., 'KmsKeyIdentifier': 'arn:aws:kms:us-west-2:<account>:alias/sim2real-xsvc-realias-<id>', 'HTTPStatusCode': 200}.

events__kms__sqs.bus-moved-from-customer-key-to-aws-owned-key-keeps-routing-by-rule-pattern — Error: Check timed out after 60.0s; the check polls DescribeEventBus for up to 60 s after update_event_bus(Name=<bus>, KmsKeyIdentifier='') for the key to disappear.

## Steps to Reproduce
1. Cause 1: events.create_event_bus(Name=..., KmsKeyIdentifier=<alias ARN no alias holds>) -> expect the alias-not-found refusal and no bus; observe 200. Then create a bus on a real key and events.update_event_bus(Name=..., KmsKeyIdentifier=<missing alias ARN>) -> expect a refusal and the bus unchanged; observe 200 with the missing alias stored.
2. Cause 2: create a bus with KmsKeyIdentifier=<symmetric key arn>, confirm DescribeEventBus reports it, then events.update_event_bus(Name=<bus>, KmsKeyIdentifier='') and poll DescribeEventBus -> expect KmsKeyIdentifier to be absent; observe it never clears.
