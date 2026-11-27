Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mgn__s3.import-inventory-creates-associated-wave-and-application mgn__s3.start-import-reused-token-other-source-rejected mgn__s3.exported-inventory-reimport-updates-application-in-place
Issue-Key: mgn__s3|StartImport|NotImplemented
Issue-Related: 20261127T193651Z-mgn-3-net-new-causes-the-whole-wave.md
Issue-Commit: 9a0283ef64a159d5cd09f281cdd10e3b4b9ba485

# mgn__s3: 2 net-new causes — StartImport is still a 501 scaffold stub (the whole S3 inventory-import flow is unreachable), and the inventory round-trip is blocked by the stuck account-initialization gate

> _Note: this issue was identified against the code base at commit `9a0283ef64a159d5cd09f281cdd10e3b4b9ba485`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartImport

## Description
Two net-new root causes behind the mgn__s3 (mgn inventory import/export over S3) checks. Each must be closed separately.

Cause 1 of 2: StartImport answers 501 NotImplemented, so no import ever runs and neither its success contract nor its idempotency-token refusal exists.
AWS: Smithy (com.amazonaws.mgn#StartImport) binds POST /StartImport with success code 202 and input clientToken (ClientIdempotencyToken, @idempotencyToken, @length 0..64), s3BucketSource (S3BucketSource, required) and tags; output is importTask (ImportTask). It models ConflictException (409), ResourceNotFoundException (404), ServiceQuotaExceededException (402), UninitializedAccountException (400) and ValidationException (400). Recorded behavior: the 202 carries an importTask whose s3BucketSource echoes the request's bucket/key and whose importID can be read until status SUCCEEDED, at which point `summary` reports per-part createdCount/modifiedCount (for a one-row sheet naming a wave and an application: waves (1,0), applications (1,0), servers (0,0)), ListImportErrors is empty, and the named wave and application exist with the application placed in that wave and carrying the row's description. Replaying the clientToken with a DIFFERENT s3BucketSource is refused with error code ConflictException and the message "Arguments on this idempotent request are inconsistent with arguments used in previous request(s)." — fixed text with no variable part — and starts nothing; replaying it with the original source returns the FIRST import's importID.
Emulator: every call answers error code NotImplemented. Replay detail: "botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartImport operation: operation StartImport is scaffolded but not implemented in this emulator". The handler is a scaffolded stub: services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/start_import.rs:162 returns crate::scaffolded::Scaffolded::not_implemented("StartImport").
Why: importing an inventory CSV from S3 is how MGN populates waves and applications in bulk. 501 is not a modeled answer, so SDK error mapping does not classify it: a client never sees an importTask, never sees the ConflictException that protects an idempotent retry from a changed source object, and cannot distinguish a rejected import from a transport failure. Because the operation produces nothing, the downstream reads (DescribeImports/ListImports, ListImportErrors, and the waves/applications the import should create) have no state to report either.
Fix: implement start_import.rs over the mgn import state — mint an importID, answer 202 with the modeled ImportTask (echoing s3BucketSource), run the sheet into the wave/application state so the task settles SUCCEEDED with the per-part created/modified summary, and record the clientToken with the request it was first used for so a replay with a different s3BucketSource is refused with ConflictException carrying AWS's fixed sentence while an unchanged replay returns the original importID; drop the SCAFFOLDED marker.
Traces: mgn__s3.import-inventory-creates-associated-wave-and-application, mgn__s3.start-import-reused-token-other-source-rejected. (Note: the sibling check mgn__s3.start-export-non-csv-key-rejected is blocked by the separately-filed StartExport stub and is NOT part of this issue.)

Cause 2 of 2: the export/re-import round trip cannot start because CreateWave refuses with UninitializedAccountException on an account whose initialization prerequisite was satisfied.
AWS: Smithy (com.amazonaws.mgn#CreateWave) binds POST /CreateWave with success code 201 and models UninitializedAccountException (400) for an account that has not been initialized. The check declares `requires:mgn-initialized`, i.e. an initialized account, and the recorded behavior there is a 201 carrying a waveID matching ^wave-[0-9a-zA-Z]{17}$, after which StartExport writes the wave and application rows to S3 and re-importing that row with a new description modifies the SAME application in place.
Emulator: HTTP 400, error code UninitializedAccountException, message "Account not initialized". Replay detail: "botocore.errorfactory.UninitializedAccountException: An error occurred (UninitializedAccountException) when calling the CreateWave operation: Account not initialized". This is the SAME root cause as Cause 1 of the companion mgn issue filed in this sweep (the account-initialization gate at services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/create_wave.rs:186 never clears, because the only path that writes `initialized_accounts` — src/ops/initialize_service.rs via src/state/account_initialization.rs — concludes in failure); it is carried here because this check is reported under the mgn__s3 heading and its replay key would otherwise be lost. Close it together with that issue's Cause 1.
Why: the data-plane round trip (export to S3, edit, re-import) never gets its fixture built, so the in-place modification semantics of an import are unverifiable and a client following the documented export/import workflow is stopped at the first control-plane write.
Fix: the account-initialization state and gate in the mgn crate — src/ops/initialize_service.rs, src/state/account_initialization.rs and the `is_initialized` gate the create handlers read — so an initialized account's CreateWave/CreateApplication succeed.
Traces: mgn__s3.exported-inventory-reimport-updates-application-in-place.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 — mgn__s3 0/4 passed, 4 failed (1 of the 4, mgn__s3.start-export-non-csv-key-rejected, is marked ALREADY FILED against the StartExport stub and is excluded from this issue).

[error] mgn__s3.import-inventory-creates-associated-wave-and-application
  ClientError: An error occurred (NotImplemented) when calling the StartImport operation: operation StartImport is scaffolded but not implemented in this emulator
  Expected (recorded AWS): HTTP 202 with importTask echoing s3BucketSource {s3Bucket, s3Key='inventory/servers.csv'}; the task settles status SUCCEEDED with summary (createdCount, modifiedCount) = waves (1,0), applications (1,0), servers (0,0); ListImportErrors empty; the named wave and application exist, the application sits in that wave and carries the row's description.
  Actual (emulator): HTTP 501, error code NotImplemented.
  Stub: services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/start_import.rs:162.

[error] mgn__s3.start-import-reused-token-other-source-rejected
  ClientError: An error occurred (NotImplemented) when calling the StartImport operation: ...
  Expected (recorded AWS): first StartImport(clientToken=T, s3Key='plan/first.csv') -> 202, settles SUCCEEDED; StartImport(clientToken=T, s3Key='plan/second.csv') -> ConflictException with message 'Arguments on this idempotent request are inconsistent with arguments used in previous request(s).' and NO new import started; StartImport(clientToken=T, s3Key='plan/first.csv') -> the first import's importID.
  Actual (emulator): HTTP 501 NotImplemented on the very first call.

[error] mgn__s3.exported-inventory-reimport-updates-application-in-place
  UninitializedAccountException: An error occurred (UninitializedAccountException) when calling the CreateWave operation: Account not initialized
  Expected (recorded AWS, initialized account): HTTP 201 with a waveID matching ^wave-[0-9a-zA-Z]{17}$, then StartExport writes the wave/application rows and the re-import modifies the same application.
  Actual (emulator): HTTP 400, code UninitializedAccountException, message 'Account not initialized'. Same root cause as Cause 1 of the companion mgn issue filed in this sweep.

Model support (services/service-com-amazonaws-mgn-applicationmigrationservice/smithy-model/com-amazonaws-mgn-applicationmigrationservice.json):
  StartImport: POST /StartImport, success 202; input clientToken(@idempotencyToken, length 0..64), s3BucketSource(required), tags; output importTask(ImportTask); errors ConflictException(409), ResourceNotFoundException(404), ServiceQuotaExceededException(402), UninitializedAccountException(400), ValidationException(400).
  CreateWave: POST /CreateWave, success 201; errors ConflictException(409), ServiceQuotaExceededException(402), UninitializedAccountException(400).

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo and boot the emulator built from that checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1, or just the three checks named in trace_ids.
3. Cause 1: create an S3 bucket, PutObject a one-row inventory CSV ('mgn:wave:name,mgn:app:name,mgn:app:description' plus one row), then call StartImport(s3BucketSource={s3Bucket, s3Key}). The emulator answers 501 NotImplemented; AWS answers 202 with an importTask that settles SUCCEEDED and creates the named wave and application (application placed in that wave).
4. Cause 1 (idempotency): call StartImport twice with the same clientToken but different s3Key. The second call must be refused ConflictException 'Arguments on this idempotent request are inconsistent with arguments used in previous request(s).' and start nothing; replaying the token with the original key must return the first importID.
5. Cause 2: with the mgn account-initialization prerequisite satisfied, call CreateWave(name=<fresh name>). The emulator answers 400 UninitializedAccountException 'Account not initialized'; AWS answers 201.
6. Re-replay mgn__s3.import-inventory-creates-associated-wave-and-application, mgn__s3.start-import-reused-token-other-source-rejected and mgn__s3.exported-inventory-reimport-updates-application-in-place; ALL THREE must pass before closing.
