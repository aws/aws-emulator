Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__kinesis__kms__s3__s3tables.s3-tables-channel-with-hmac-destination-key-rejected
Issue-Key: glue__iam__kinesis__kms__s3__s3tables.s3-tables-channel-with-hmac-destination-key-rejected|_refused#22|AssertionError|raise AssertionError("CreateChannel accepted an HMAC_256 key as the delivery encryption key")
Issue-Commit: 47d3968c23878270cbb97f1f35a0ca51e5d34ce1

# glue__iam__kinesis__kms__s3__s3tables: Kinesis CreateChannel accepts an HMAC KMS key as the delivery encryption key instead of refusing it with ValidationException

> _Note: this issue was identified against the code base at commit `47d3968c23878270cbb97f1f35a0ca51e5d34ce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
Cause 1 of 1: CreateChannel does not validate the KeySpec/KeyUsage of the KMS key named in EncryptionConfiguration.
AWS: a Kinesis-to-S3-Tables delivery channel may only be encrypted with a symmetric encryption CMK. A CreateChannel naming a key whose KeySpec is HMAC_256 and whose KeyUsage is GENERATE_VERIFY_MAC is refused 400 `ValidationException` with the message `The KMS key specified in EncryptionConfiguration.KeyId must be a symmetric encryption key with KeySpec SYMMETRIC_DEFAULT and key usage ENCRYPT_DECRYPT. Asymmetric and HMAC keys are not supported.` — that sentence is entirely fixed text; only the member name it cites (EncryptionConfiguration.KeyId) comes from the request shape. ValidationException is modeled on the operation (Smithy: Kinesis CreateChannel errors include ValidationException, plus the KMS* family), and the identical request naming a SYMMETRIC_DEFAULT/ENCRYPT_DECRYPT customer managed key goes ACTIVE and echoes the key (that positive control passes today). Reference: https://docs.aws.amazon.com/streams/latest/dev/data-delivery-security.html
Emulator: the HMAC-key request is accepted — CreateChannel returns 200 with ChannelDescription.ChannelARN and the channel is left on the stream.
Why: a client is allowed to create a channel real AWS rejects, so a misconfigured key only surfaces against production (and nothing in the emulator ever exercises the ValidationException path). It also leaves a channel on the stream that AWS would never have created, which diverges every later DescribeChannel/ListChannels read.
Fix: input validation in the CreateChannel handler of the kinesis service crate — it must resolve EncryptionConfiguration.KeyId through KMS and refuse any key that is not KeySpec SYMMETRIC_DEFAULT with KeyUsage ENCRYPT_DECRYPT before the channel is created.
Traces: glue__iam__kinesis__kms__s3__s3tables.s3-tables-channel-with-hmac-destination-key-rejected.

## Evidence
`AssertionError: CreateChannel accepted an HMAC_256 key as the delivery encryption key` — raised because kinesis.create_channel(**request) returned ChannelDescription.ChannelARN instead of raising. Expected (code, message) = ('ValidationException', 'The KMS key specified in EncryptionConfiguration.KeyId must be a symmetric encryption key with KeySpec SYMMETRIC_DEFAULT and key usage ENCRYPT_DECRYPT. Asymmetric and HMAC keys are not supported.'). The same request shape with a SYMMETRIC_DEFAULT/ENCRYPT_DECRYPT key is accepted and reaches ChannelStatus ACTIVE in the same check, so only the key-spec validation is missing.

## Steps to Reproduce
1. kms:CreateKey with KeySpec=SYMMETRIC_DEFAULT, KeyUsage=ENCRYPT_DECRYPT (good key) and a second with KeySpec=HMAC_256, KeyUsage=GENERATE_VERIFY_MAC (mac key).
2. glue:CreateRegistry + CreateSchema (the GSR schema ARN), s3:CreateBucket (DLQ), s3tables:CreateTableBucket + CreateNamespace, kinesis:CreateStream (ON_DEMAND, wait ACTIVE), iam:CreateRole trusting kinesis.amazonaws.com with access to those resources and both keys.
3. kinesis:CreateChannel with ChannelName, ServiceExecutionRoleARN, StreamConfigurationList=[{StreamARN, RecordConfiguration={RecordFormatType JSON, GSRSchemaARN}}], the S3 Tables destination and EncryptionConfiguration naming the HMAC_256 key ARN.
4. Observe 200 with a ChannelARN; AWS refuses with ValidationException and leaves no channel on the stream.
