Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: databrew.list-job-runs-pages-two-profile-runs-one-per-page databrew.profile-job-run-past-its-timeout-ends-timeout databrew.profile-job-run-statistics-land-in-s3-and-stop-after-finish-is-a-no-op databrew.profile-job-writes-profile-under-updated-output-location databrew.start-job-run-rejects-second-run-while-first-is-active
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew|CreateProfileJob|NotImplemented
Issue-Commit: 8d1ea8adc587d6ef43eb599364c1f7e78d6ff3ea

# Implement databrew CreateProfileJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8d1ea8adc587d6ef43eb599364c1f7e78d6ff3ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProfileJob

## Description
`CreateProfileJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/create_profile_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `CreateProfileJob` call, so each one unblocks only when `CreateProfileJob` works: `databrew.list-job-runs-pages-two-profile-runs-one-per-page`, `databrew.profile-job-run-past-its-timeout-ends-timeout`, `databrew.profile-job-run-statistics-land-in-s3-and-stop-after-finish-is-a-no-op`, `databrew.profile-job-writes-profile-under-updated-output-location`, `databrew.start-job-run-rejects-second-run-while-first-is-active`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/create_profile_job.rs`. Replaying the corpus, the emulator refused the `CreateProfileJob` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateProfileJob operation: operation CreateProfileJob is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the databrew emulator
Call CreateProfileJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
