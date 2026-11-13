Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.batch-get-secret-value-binary-secret-returns-raw-bytes secretsmanager.batch-get-secret-value-filters-form-and-pagination secretsmanager.batch-get-secret-value-order-independent-of-input secretsmanager.batch-get-secret-value-partial-error-for-secret-marked-for-deletion secretsmanager.batch-get-secret-value-returns-partial-results-with-a-per-secret-error-for-a-denied-secret secretsmanager.get-secret-value-on-secret-created-without-value-rejected-until-first-put
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secrets-manager|BatchGetSecretValue|NotImplemented
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# Implement secretsmanager BatchGetSecretValue: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetSecretValue

## Description
`BatchGetSecretValue` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/batch_get_secret_value.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `BatchGetSecretValue` call, so each one unblocks only when `BatchGetSecretValue` works: `secretsmanager.batch-get-secret-value-binary-secret-returns-raw-bytes`, `secretsmanager.batch-get-secret-value-filters-form-and-pagination`, `secretsmanager.batch-get-secret-value-order-independent-of-input`, `secretsmanager.batch-get-secret-value-partial-error-for-secret-marked-for-deletion`, `secretsmanager.batch-get-secret-value-returns-partial-results-with-a-per-secret-error-for-a-denied-secret`, `secretsmanager.get-secret-value-on-secret-created-without-value-rejected-until-first-put`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/batch_get_secret_value.rs`. Replaying the corpus, the emulator refused the `BatchGetSecretValue` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the BatchGetSecretValue operation: operation BatchGetSecretValue is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the secretsmanager emulator
Call BatchGetSecretValue and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
