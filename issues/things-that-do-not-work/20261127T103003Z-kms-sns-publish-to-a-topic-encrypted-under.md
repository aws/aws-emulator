Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__sns.publish-on-a-topic-under-an-asymmetric-key-rejected
Issue-Key: kms__sns.publish-on-a-topic-under-an-asymmetric-key-rejected|run#7|AssertionError|raise AssertionError("Publish on a topic encrypted under the RSA_2048 key unexpectedly succeeded")
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# kms__sns: Publish to a topic encrypted under an asymmetric key succeeds instead of ParameterValueInvalid

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
One root cause, grounded in a failing behavioral check. The open issue 20261117T122437Z-kms-sns-publish-to-a-topic-whose-kmsmasterkeyid.md covers the nonexistent-alias case (KMSNotFound); this is the key-usage case and is not covered there.

Cause 1 of 1: Publish does not resolve the topic's KmsMasterKeyId before encrypting, so a topic whose key is asymmetric accepts messages.
  AWS: recorded behavior only. Publishing to a topic whose KmsMasterKeyId names an RSA_2048 key is refused 400 ParameterValueInvalid whose message relays KMS's refusal and ends '(Service: AWSKMS; Status Code: 400; Error Code: InvalidKeyUsageException; Request ID: <id>; Proxy: null)' — fixed text with the key named from the topic's attribute and <id> per call. After SetTopicAttributes moves the topic to a symmetric key, the same Publish succeeds with a UUID MessageId.
  Emulator: the publish succeeds — 'Publish on a topic encrypted under the RSA_2048 key unexpectedly succeeded'.
  Why: a client is told its message was published to an encrypted topic whose key cannot generate a data key; the refusal AWS gives is never produced, so a mis-keyed topic looks healthy and message encryption cannot be relied on.
  Fix: the sns Publish handler's SSE path — resolve the topic's KmsMasterKeyId through KMS, require a symmetric encryption key, and relay KMS's InvalidKeyUsageException as ParameterValueInvalid.
  Traces: kms__sns.publish-on-a-topic-under-an-asymmetric-key-rejected.

## Evidence
kms__sns.publish-on-a-topic-under-an-asymmetric-key-rejected —
File "check_...py", line 43, in run -> AssertionError: Publish on a topic encrypted under the RSA_2048 key unexpectedly succeeded.
Expected ('ParameterValueInvalid', <KMS relay ending '(Service: AWSKMS; Status Code: 400; Error Code: InvalidKeyUsageException; Request ID: <id>; Proxy: null)'>).

## Steps to Reproduce
1. Create an RSA_2048 ENCRYPT_DECRYPT key and a symmetric key.
2. Create an SNS topic and set KmsMasterKeyId to the RSA key's ARN.
3. sns.publish(TopicArn=..., Message='...') -> expect 400 ParameterValueInvalid relaying KMS's InvalidKeyUsageException; observe success.
4. SetTopicAttributes to the symmetric key and publish again -> succeeds with a UUID MessageId.
