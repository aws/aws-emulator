Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__sns__sqs.fifo-topic-under-an-ecc-signing-key-rejects-publish-while-its-symmetric-twin-delivers
Issue-Key: kms__sns__sqs.fifo-topic-under-an-ecc-signing-key-rejects-publish-while-its-symmetric-twin-delivers|_refused#20|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {out!r}")
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# kms__sns__sqs: Publish to a FIFO topic encrypted under an ECC signing key succeeds instead of being refused

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
One root cause, grounded in a failing behavioral check.

Cause 1 of 1: Publish to a FIFO topic does not resolve the topic's KMS key, so an ECC_NIST_P256 SIGN_VERIFY key is accepted as the topic's encryption key.
  AWS: recorded behavior only. A FIFO topic whose KmsMasterKeyId names an ECC signing key cannot have a data key generated for it, so Publish is refused 400 with the relayed KMS failure ending '(Service: AWSKMS; Status Code: 400; Error Code: InvalidKeyUsageException; Request ID: <id>; Proxy: null)' — fixed text with the key from the topic attribute and <id> per call — and nothing is delivered to the subscribed queue; the symmetric twin topic publishes and delivers normally.
  Emulator: the publish succeeds, answering {'MessageId': <uuid>, 'SequenceNumber': '10000000000000003000', HTTPStatusCode 200}.
  Why: a client is told a FIFO message was published under a key that cannot encrypt it; the refusal AWS produces never happens, and ordering/encryption guarantees on the topic are not what the configuration implies.
  Fix: the sns Publish handler's SSE path for FIFO topics — resolve KmsMasterKeyId through KMS, require a symmetric encryption key, and relay KMS's InvalidKeyUsageException.
  Traces: kms__sns__sqs.fifo-topic-under-an-ecc-signing-key-rejects-publish-while-its-symmetric-twin-delivers.

## Evidence
kms__sns__sqs.fifo-topic-under-an-ecc-signing-key-rejects-publish-while-its-symmetric-twin-delivers —
AssertionError: the call was expected to be refused but returned {'MessageId': 'de8de079-...', 'SequenceNumber': '10000000000000003000', 'HTTPStatusCode': 200}.
Expected a 400 refusal whose message relays KMS's InvalidKeyUsageException for the ECC_NIST_P256 key.

## Steps to Reproduce
1. Create an ECC_NIST_P256 SIGN_VERIFY key and a symmetric key.
2. Create two FIFO topics, one with KmsMasterKeyId = the ECC key, one with the symmetric key, each subscribed to a FIFO queue.
3. sns.publish(TopicArn=<ecc topic>, Message=..., MessageGroupId='g1') -> expect the 400 KMS relay and no delivery; observe a MessageId.
4. Publish to the symmetric twin -> succeeds and delivers.
