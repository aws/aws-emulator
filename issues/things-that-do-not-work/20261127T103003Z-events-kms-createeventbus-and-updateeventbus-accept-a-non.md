Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__kms.create-event-bus-with-an-asymmetric-or-hmac-key-rejected-while-a-symmetric-key-is-accepted events__kms.create-event-bus-by-an-alias-of-an-rsa-key-rejected-until-the-alias-points-at-a-symmetric-key events__kms.update-event-bus-to-an-rsa-key-rejected-and-the-bus-keeps-its-current-encryption
Issue-Key: events__kms.create-event-bus-with-an-asymmetric-or-hmac-key-rejected-while-a-symmetric-key-is-accepted|_refusal#12|AssertionError|raise AssertionError(f"CreateEventBus accepted a non-symmetric key: {made!r}")
Issue-Key: events__kms.create-event-bus-by-an-alias-of-an-rsa-key-rejected-until-the-alias-points-at-a-symmetric-key|run#13|AssertionError|raise AssertionError(f"CreateEventBus accepted an alias of an RSA key: {made!r}")
Issue-Key: events__kms.update-event-bus-to-an-rsa-key-rejected-and-the-bus-keeps-its-current-encryption|_refused#12|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded: {answer!r}")
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# events__kms: CreateEventBus and UpdateEventBus accept a non-symmetric KmsKeyIdentifier (directly or through an alias)

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventBus

## Description
One root cause, grounded in three failing behavioral checks.

Cause 1 of 1: the KmsKeyIdentifier of an event bus is never resolved to a key spec, so asymmetric and HMAC keys — and aliases pointing at them — are accepted on both create and update.
  AWS: Smithy: CreateEventBus.KmsKeyIdentifier documents 'The identifier of the KMS customer managed key for EventBridge to use ... can be the key ARN, KeyId, key alias, or key alias ARN', and EventBridge requires a symmetric encryption key. The recorded refusal is 400 ValidationException with the message 'Parameter <identifier> is not valid. Reason: Specified KMS Key is not symmetric.' — fixed text 'Parameter ', ' is not valid. Reason: Specified KMS Key is not symmetric.' with <identifier> exactly as supplied in the request (key ARN or alias ARN). A refused CreateEventBus leaves no bus: DescribeEventBus answers ('ResourceNotFoundException', 'Event bus <name> does not exist.'). A refused UpdateEventBus leaves the bus on its current key.
  Emulator: all three calls return 200 — CreateEventBus with an RSA/HMAC key ARN, CreateEventBus with an alias of an RSA key, and UpdateEventBus to an RSA key — each echoing KmsKeyIdentifier as given, and the bus is created/re-keyed.
  Why: a client is allowed to bind a bus to a key that can never wrap a data key; the configuration error AWS reports synchronously instead surfaces (if at all) as unencryptable events later, and the refused-create invariant (no bus left behind) does not hold.
  Fix: the eventbridge CreateEventBus and UpdateEventBus handlers' KmsKeyIdentifier validation — resolve key ARNs, key ids and aliases through KMS and require a symmetric encryption key spec before the bus is created or re-keyed.
  Traces: events__kms.create-event-bus-with-an-asymmetric-or-hmac-key-rejected-while-a-symmetric-key-is-accepted, events__kms.create-event-bus-by-an-alias-of-an-rsa-key-rejected-until-the-alias-points-at-a-symmetric-key, events__kms.update-event-bus-to-an-rsa-key-rejected-and-the-bus-keeps-its-current-encryption.

## Evidence
events__kms.create-event-bus-with-an-asymmetric-or-hmac-key-rejected-while-a-symmetric-key-is-accepted — AssertionError: CreateEventBus accepted a non-symmetric key: {'EventBusArn': ..., 'KmsKeyIdentifier': 'arn:aws:kms:...:key/fb6efd72-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}. Expected ('ValidationException', 'Parameter <key arn> is not valid. Reason: Specified KMS Key is not symmetric.').

events__kms.create-event-bus-by-an-alias-of-an-rsa-key-rejected-until-the-alias-points-at-a-symmetric-key — AssertionError: CreateEventBus accepted an alias of an RSA key: {..., 'KmsKeyIdentifier': 'arn:aws:kms:...:alias/sim2real-xsvc-busalias-...'}. Expected the same ValidationException naming the alias ARN, and DescribeEventBus of the refused bus answering ('ResourceNotFoundException', 'Event bus <name> does not exist.').

events__kms.update-event-bus-to-an-rsa-key-rejected-and-the-bus-keeps-its-current-encryption — AssertionError: expected a refusal, the call succeeded: {'Arn': ..., 'KmsKeyIdentifier': 'arn:aws:kms:...:key/36252ea2-...', 'HTTPStatusCode': 200}.

## Steps to Reproduce
1. Create an RSA_2048 key, an HMAC_256 key, a symmetric key, and an alias pointing at the RSA key.
2. events.create_event_bus(Name=..., KmsKeyIdentifier=<rsa key arn>) and (…, KmsKeyIdentifier=<hmac key arn>) -> expect 400 ValidationException 'Parameter <arn> is not valid. Reason: Specified KMS Key is not symmetric.'; observe 200.
3. events.create_event_bus(Name=..., KmsKeyIdentifier=<alias arn of the RSA key>) -> expect the same refusal; observe 200.
4. Create a bus on the symmetric key, then events.update_event_bus(Name=..., KmsKeyIdentifier=<rsa key arn>) -> expect the same refusal and the bus unchanged; observe 200 with the RSA key stored.
