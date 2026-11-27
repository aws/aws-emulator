Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__logs.create-log-group-with-rsa-key-rejected-and-group-absent kms__logs.associate-kms-key-with-rsa-key-rejected-symmetric-twin-encrypts-events kms__logs.query-result-association-with-rsa-key-rejected-queries-unaffected kms__logs.create-lookup-table-with-cmk-scoped-to-other-table-rejected kms__logs.update-lookup-table-to-ungranted-cmk-rejected-until-granted
Issue-Key: kms__logs.create-log-group-with-rsa-key-rejected-and-group-absent|_code_and_message#10|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded with {answer!r}")
Issue-Key: kms__logs.associate-kms-key-with-rsa-key-rejected-symmetric-twin-encrypts-events|_refusal#14|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {out!r}")
Issue-Key: kms__logs.query-result-association-with-rsa-key-rejected-queries-unaffected|_denied#16|AssertionError|raise AssertionError(f"the association was accepted, answering {reply!r}")
Issue-Key: kms__logs.create-lookup-table-with-cmk-scoped-to-other-table-rejected|_error_of#12|AssertionError|raise AssertionError(f"expected a refusal, got a successful response {resp!r}")
Issue-Key: kms__logs.update-lookup-table-to-ungranted-cmk-rejected-until-granted|_refused#13|AssertionError|raise AssertionError(f"the call should have been refused; it returned {answer!r}")
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# kms__logs: 2 causes — a non-encrypt/decrypt KMS key is accepted everywhere kmsKeyId is taken, and lookup-table CMK authorization is never evaluated

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateKmsKey

## Description
Two distinct root causes, grounded in five failing behavioral checks.

Cause 1 of 2: CloudWatch Logs accepts an asymmetric key wherever a kmsKeyId is taken.
  AWS: Smithy: CreateLogGroup.kmsKeyId and AssociateKmsKey.kmsKeyId (log group or resourceIdentifier) are plain strings resolved against KMS, and both declare InvalidParameterException. Recorded behavior: an RSA key (any of RSA_2048/3072/4096) is refused 400 InvalidParameterException with the message 'Invalid key usage specified, only encrypt/decrypt are supported.' — fixed text, no variable parts. A refused CreateLogGroup leaves no group (CreateLogStream on it answers ('ResourceNotFoundException', 'The specified log group does not exist.')); a refused AssociateKmsKey leaves the target's key unchanged (no key, or the symmetric key it already had); and the query-results resource reports no encryptionKey afterwards.
  Emulator: all three calls return 200 with an empty body — CreateLogGroup with an RSA key, AssociateKmsKey of an RSA key with a log group (both before and after a symmetric key is associated), and AssociateKmsKey of an RSA key with the account's query-results resource.
  Why: a client is allowed to bind log data to a key that can never wrap a data key; the InvalidParameterException AWS raises at association time never comes, and a previously good symmetric association can be overwritten by an unusable key.
  Fix: the logs CreateLogGroup and AssociateKmsKey handlers' kmsKeyId validation — resolve the key in KMS and refuse any key whose usage is not ENCRYPT_DECRYPT with that fixed message.
  Traces: kms__logs.create-log-group-with-rsa-key-rejected-and-group-absent, kms__logs.associate-kms-key-with-rsa-key-rejected-symmetric-twin-encrypts-events, kms__logs.query-result-association-with-rsa-key-rejected-queries-unaffected.

Cause 2 of 2: lookup-table operations never evaluate whether Logs may use the named CMK.
  AWS: recorded behavior only. CreateLookupTable and UpdateLookupTable check that the key policy lets CloudWatch Logs use the key for that table's encryption context, and refuse otherwise with 400 AccessDeniedException whose message starts 'Insufficient permissions to use KMS key '' and ends with the key id (quoted) — fixed text with the key id from the request. Nothing is created or changed: GetLookupTable for the refused table answers ResourceNotFoundException "Lookup table '<name>' not found in account '<account>'", and a refused re-key leaves the table's rows and original kmsKeyId intact. Once the key policy grants Logs that table, the identical call succeeds.
  Emulator: both calls return 200 — CreateLookupTable under a key scoped to a different table returns {'lookupTableArn': ..., 'createdAt': ...} and the stray table exists; UpdateLookupTable to a key Logs holds no grant on returns {'lookupTableArn': ..., 'lastUpdatedTime': ...} and re-keys the table.
  Why: key policies scoped per lookup table have no effect — a table can be created or re-keyed onto a key its owner never authorized, and the AccessDeniedException AWS uses to signal that is never produced.
  Fix: the logs CreateLookupTable and UpdateLookupTable handlers' KMS authorization step — evaluate the key policy for the logs service principal with the table's encryption context before writing, and refuse with the 'Insufficient permissions to use KMS key ...' AccessDeniedException.
  Traces: kms__logs.create-lookup-table-with-cmk-scoped-to-other-table-rejected, kms__logs.update-lookup-table-to-ungranted-cmk-rejected-until-granted.

## Evidence
kms__logs.create-log-group-with-rsa-key-rejected-and-group-absent — AssertionError: expected a refusal, the call succeeded with {'HTTPStatusCode': 200, 'content-length': '2', ...}. Expected ('InvalidParameterException', 'Invalid key usage specified, only encrypt/decrypt are supported.').
kms__logs.associate-kms-key-with-rsa-key-rejected-symmetric-twin-encrypts-events — AssertionError: the call was expected to be refused but returned {'HTTPStatusCode': 200, ...}; same expected tuple.
kms__logs.query-result-association-with-rsa-key-rejected-queries-unaffected — AssertionError: the association was accepted, answering {'HTTPStatusCode': 200, ...}; same expected tuple.

kms__logs.create-lookup-table-with-cmk-scoped-to-other-table-rejected — AssertionError: expected a refusal, got a successful response {'lookupTableArn': 'arn:aws:logs:us-west-2:<account>:lookup-table:sim2real_xsvc_ltno_<id>', 'createdAt': ...}. Expected AccessDeniedException "Insufficient permissions to use KMS key '<key id>'".
kms__logs.update-lookup-table-to-ungranted-cmk-rejected-until-granted — AssertionError: the call should have been refused; it returned {'lookupTableArn': ..., 'lastUpdatedTime': ...}. Same expected refusal, with the table left on its original key and rows.

## Steps to Reproduce
1. Cause 1: create RSA and symmetric keys with a policy allowing logs.<region>.amazonaws.com. logs.create_log_group(logGroupName=..., kmsKeyId=<rsa>) -> expect InvalidParameterException 'Invalid key usage specified, only encrypt/decrypt are supported.'; observe 200. logs.associate_kms_key(logGroupName=<group>, kmsKeyId=<rsa>) and logs.associate_kms_key(resourceIdentifier=<account query-results arn>, kmsKeyId=<rsa>) -> same expectation; observe 200.
2. Cause 2: create a key whose policy allows Logs only for one lookup table's encryption context; logs.create_lookup_table(lookupTableName=<other>, tableBody=..., kmsKeyId=<that key>) -> expect AccessDeniedException "Insufficient permissions to use KMS key '<key id>'" and no table; observe a created table. Then logs.update_lookup_table(lookupTableArn=<existing>, kmsKeyId=<a key with no Logs grant>) -> expect the same refusal and the table unchanged; observe a re-keyed table.
