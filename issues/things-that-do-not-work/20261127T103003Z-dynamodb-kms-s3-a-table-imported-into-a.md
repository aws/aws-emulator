Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kms__s3.import-table-into-customer-key-table-reads-back-s3-items
Issue-Key: dynamodb__kms__s3.import-table-into-customer-key-table-reads-back-s3-items|_poll#27|AssertionError|raise AssertionError(f"{what}: last read was {value!r}")
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# dynamodb__kms__s3: a table imported into a customer-key table holds no KMS grant constrained to it

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportTable

## Description
One root cause, grounded in a failing behavioral check.

Cause 1 of 1: ImportTable into a table encrypted with a customer managed key leaves no grant on that key.
  AWS: recorded behavior only. The table created by ImportTable under an SSESpecification naming a customer managed key carries a KMS grant for the dynamodb.amazonaws.com service principal constrained to that table (encryption-context-subset carrying the table ARN); ListGrants on the key returns it while the table exists.
  Emulator: ListGrants on the key keeps returning [] — 'the customer key holds no grant constrained to the imported table: last read was []'.
  Why: a client auditing or revoking the service's access to its CMK for an imported table has nothing to audit or revoke, and the key/table binding AWS exposes is absent.
  Fix: the dynamodb ImportTable handler's SSE path (mint the table-constrained service grant on the key) and the KMS grant store it writes into. Same underlying grant-minting gap as CreateTable (filed for dynamodb__kms).
  Traces: dynamodb__kms__s3.import-table-into-customer-key-table-reads-back-s3-items.

## Evidence
dynamodb__kms__s3.import-table-into-customer-key-table-reads-back-s3-items —
File "check_...py", line 185, in run -> AssertionError: the customer key holds no grant constrained to the imported table: last read was [].

## Steps to Reproduce
1. Create a symmetric CMK and an S3 bucket holding newline-delimited JSON items.
2. dynamodb.import_table(S3BucketSource=..., InputFormat='DYNAMODB_JSON', TableCreationParameters={... 'SSESpecification': {'Enabled': True, 'SSEType': 'KMS', 'KMSMasterKeyId': <key arn>}}).
3. Wait for the import to complete and poll kms.list_grants(KeyId=<key>).
4. Observe [] throughout; AWS shows a dynamodb grant constrained to the imported table.
