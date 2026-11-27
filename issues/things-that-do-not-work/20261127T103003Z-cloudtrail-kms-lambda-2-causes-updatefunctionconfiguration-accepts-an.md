Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__kms__lambda.durable-config-update-naming-an-asymmetric-kms-key-rejected-and-audited cloudtrail__kms__lambda.durable-kms-key-set-on-update-is-probed-by-dry-run-generate-data-key-and-decrypt
Issue-Key: cloudtrail__kms__lambda.durable-config-update-naming-an-asymmetric-kms-key-rejected-and-audited|_refusal#13|AssertionError|raise AssertionError(f"expected the update to be refused, but it answered members {sorted(got)!r}")
Issue-Key: cloudtrail__kms__lambda.durable-kms-key-set-on-update-is-probed-by-dry-run-generate-data-key-and-decrypt|_trail#13|AssertionError|raise AssertionError(
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# cloudtrail__kms__lambda: 2 causes — UpdateFunctionConfiguration accepts an asymmetric DurableConfig.KMSKeyArn, and the durable KMS probe writes no CloudTrail records

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionConfiguration

## Description
Two distinct root causes, each grounded in a failing behavioral check.

Cause 1 of 2: UpdateFunctionConfiguration accepts a DurableConfig.KMSKeyArn naming an asymmetric (RSA_2048) KMS key.
  AWS: recorded behavior only (the Lambda model constrains DurableConfig.KMSKeyArn only as an ARN string). Durable Functions require a symmetric encryption key, so the update is refused 400 InvalidParameterValueException with the message 'The DurableConfig.KMSKeyArn value <key arn> is not a symmetric key (key spec: RSA_2048). Durable Functions require a symmetric KMS key.' — fixed text with <key arn> from the request and the key spec read from KMS — and the function keeps no KMSKeyArn in its DurableConfig.
  Emulator: the call returns 200 with a full function-configuration response (members Architectures ... Version, including DurableConfig), i.e. the asymmetric key is stored.
  Why: a client that would be told its key is unusable instead gets a function configured against a key that can never wrap a data key; the failure surfaces later, at invoke time, instead of at configuration time.
  Fix: the UpdateFunctionConfiguration handler's DurableConfig validation — resolve KMSKeyArn against KMS and require a symmetric encryption key spec before storing it.
  Traces: cloudtrail__kms__lambda.durable-config-update-naming-an-asymmetric-kms-key-rejected-and-audited.

Cause 2 of 2: the KMS calls Lambda makes when a durable KMS key is set are never recorded in CloudTrail.
  AWS: recorded behavior only. Setting DurableConfig.KMSKeyArn makes Lambda probe the key, and those calls appear in CloudTrail (LookupEvents/the key's event history) as GenerateDataKey (dry run) and Decrypt events attributed to the Lambda service principal for that key resource, within the delivery window.
  Emulator: the check's CloudTrail delivery wait expires with zero matching records — 'CloudTrail held 0 matching record(s) when its delivery wait ran out: []'.
  Why: a client auditing which principal used a CMK, or asserting that a service really touched the key, sees an empty trail; key-usage attribution is unobservable.
  Fix: the CloudTrail event sink for KMS data-plane operations (the KMS handlers' audit emission and CloudTrail's LookupEvents store) so service-principal GenerateDataKey/Decrypt calls are recorded.
  Traces: cloudtrail__kms__lambda.durable-kms-key-set-on-update-is-probed-by-dry-run-generate-data-key-and-decrypt.

## Evidence
cloudtrail__kms__lambda.durable-config-update-naming-an-asymmetric-kms-key-rejected-and-audited —
AssertionError: expected the update to be refused, but it answered members ['Architectures', 'CodeSha256', 'CodeSize', 'Description', 'DurableConfig', 'EphemeralStorage', 'FunctionArn', 'FunctionName', 'Handler', 'LastModified', 'LastUpdateStatus', ..., 'Version'].
Expected: 400 InvalidParameterValueException 'The DurableConfig.KMSKeyArn value <key arn> is not a symmetric key (key spec: RSA_2048). Durable Functions require a symmetric KMS key.'

cloudtrail__kms__lambda.durable-kms-key-set-on-update-is-probed-by-dry-run-generate-data-key-and-decrypt —
AssertionError: CloudTrail held 0 matching record(s) when its delivery wait ran out: []. Expected: a GenerateDataKey (DryRun) and a Decrypt record for the configured key, attributed to the Lambda service principal.

## Steps to Reproduce
1. Create an RSA_2048 ENCRYPT_DECRYPT key and a symmetric key; create a durable-execution Lambda function.
2. Cause 1: lambda.update_function_configuration(FunctionName=<fn>, DurableConfig={'ExecutionTimeout':300,'RetentionPeriodInDays':1,'KMSKeyArn':<rsa arn>}) -> expect 400 InvalidParameterValueException with the symmetric-key message; observe 200 with DurableConfig stored.
3. Cause 2: set DurableConfig.KMSKeyArn to the symmetric key, then poll cloudtrail.lookup_events for GenerateDataKey/Decrypt on that key -> expect both records; observe none.
