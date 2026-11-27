Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kinesis__kms__s3.create-channel-with-asymmetric-destination-key-rejected
Issue-Key: iam__kinesis__kms__s3.create-channel-with-asymmetric-destination-key-rejected|_refusal#16|AssertionError|raise AssertionError("CreateChannel accepted an RSA_2048 key as the delivery encryption key")
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# iam__kinesis__kms__s3: CreateChannel accepts an RSA_2048 key as the delivery encryption key

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
One root cause, grounded in a failing behavioral check.

Cause 1 of 1: the channel's EncryptionConfiguration KeyId is not resolved to a key spec, so an asymmetric (RSA_2048) key is accepted.
  AWS: recorded behavior only (the model types the key id as a string). Delivery encryption requires a symmetric encryption key, so CreateChannel naming an RSA_2048 key is refused with a 400 client error naming the key and creates no channel; the same request with a symmetric key is accepted.
  Emulator: CreateChannel succeeds — 'CreateChannel accepted an RSA_2048 key as the delivery encryption key'.
  Why: a client is allowed to create a delivery channel bound to a key that cannot generate a data key; the configuration error AWS raises at create time becomes a silent data-path failure.
  Fix: the CreateChannel handler's EncryptionConfiguration validation — resolve the key in KMS and require a symmetric encryption key spec.
  Traces: iam__kinesis__kms__s3.create-channel-with-asymmetric-destination-key-rejected.

## Evidence
iam__kinesis__kms__s3.create-channel-with-asymmetric-destination-key-rejected —
File "check_...py", line 83, in run -> AssertionError: CreateChannel accepted an RSA_2048 key as the delivery encryption key.

## Steps to Reproduce
1. Create an RSA_2048 ENCRYPT_DECRYPT key and a symmetric key, plus the delivery role and S3 destination.
2. CreateChannel with EncryptionConfiguration naming the RSA key -> expect a 400 refusal and no channel; observe the channel created.
3. Repeat with the symmetric key -> accepted.
