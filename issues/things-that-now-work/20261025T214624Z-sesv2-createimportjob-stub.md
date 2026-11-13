Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.import-job-create-get-list-lifecycle
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|CreateImportJob|NotImplemented
Issue-Commit: ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8

# Implement sesv2 CreateImportJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateImportJob

## Description
`CreateImportJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_import_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateImportJob` call, so each one unblocks only when `CreateImportJob` works: `sesv2.import-job-create-get-list-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_import_job.rs`. Replaying the corpus, the emulator refused the `CreateImportJob` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_sesv2_import_job_create_get_list_lifecycle.py", line 37, in run
    assert error.get("Code") == "BadRequestException", f"unexpected CreateImportJob error: {error!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: unexpected CreateImportJob error: {'Message': 'operation CreateImportJob is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the sesv2 emulator
Call CreateImportJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
