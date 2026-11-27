Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__kms.create-channel-asymmetric-kms-key-rejected-symmetric-twin-active
Issue-Key: kinesis__kms.create-channel-asymmetric-kms-key-rejected-symmetric-twin-active|_refusal#36|AssertionError|raise AssertionError(f"expected CreateChannel to be refused, but it returned {result!r}")
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# kinesis__kms: CreateChannel accepts an asymmetric KMS key as the delivery encryption key

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
One root cause, grounded in a failing behavioral check.

Cause 1 of 1: the channel's EncryptionConfiguration KeyId is not resolved to a key spec, so an RSA key is accepted and the channel goes to CREATING.
  AWS: recorded behavior only (the model types KeyId as a string). Channel delivery encryption requires a symmetric encryption key, so CreateChannel naming an RSA key is refused with a 400 client error naming the key and creates no channel, while its symmetric twin is accepted and becomes ACTIVE.
  Emulator: CreateChannel returns the full channel representation — ChannelStatus 'CREATING' with EncryptionConfiguration {'EncryptionType': 'KMS', 'KeyId': '<rsa key arn>'}.
  Why: a client is allowed to create a channel whose encryption key cannot generate a data key; the error AWS reports at create time turns into a silent delivery failure.
  Fix: the CreateChannel handler's EncryptionConfiguration validation — resolve the key through KMS and require a symmetric encryption key spec.
  Traces: kinesis__kms.create-channel-asymmetric-kms-key-rejected-symmetric-twin-active.

## Evidence
kinesis__kms.create-channel-asymmetric-kms-key-rejected-symmetric-twin-active —
AssertionError: expected CreateChannel to be refused, but it returned {'ChannelName': 'sim2real-xsvc-dlvrsa-<id>', 'ChannelStatus': 'CREATING', ..., 'EncryptionConfiguration': {'EncryptionType': 'KMS', 'KeyId': 'arn:aws:kms:us-west-2:<account>:key/789fb483-...'}}.

## Steps to Reproduce
1. Create an RSA key and a symmetric key, a delivery role, a source stream and an S3 destination bucket.
2. kinesis.create_channel(..., EncryptionConfiguration={'EncryptionType': 'KMS', 'KeyId': <rsa key arn>}) -> expect a 400 refusal and no channel; observe a CREATING channel.
3. Repeat with the symmetric key -> the channel becomes ACTIVE.
