Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.cloud-connector-delete-nonexistent-rejected ssm.cloud-connector-get-nonexistent-not-found ssm.cloud-connector-list-reflects-created-connectors ssm.cloud-connector-update-nonexistent-rejected ssm.create-cloud-connector-quota-exceeded-blocked ssm.validate-cloud-connector-nonexistent-rejected ssm.validate-cloud-connector-unknown-id-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|ListCloudConnectors|NotImplemented
Issue-Commit: 69e3918bc598d03b2c669434388419738bf97909

# Implement ssm ListCloudConnectors: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `69e3918bc598d03b2c669434388419738bf97909`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCloudConnectors

## Description
`ListCloudConnectors` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssm-amazonssm/src/ops/list_cloud_connectors.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `ListCloudConnectors` call, so each one unblocks only when `ListCloudConnectors` works: `ssm.cloud-connector-delete-nonexistent-rejected`, `ssm.cloud-connector-get-nonexistent-not-found`, `ssm.cloud-connector-list-reflects-created-connectors`, `ssm.cloud-connector-update-nonexistent-rejected`, `ssm.create-cloud-connector-quota-exceeded-blocked`, `ssm.validate-cloud-connector-nonexistent-rejected`, `ssm.validate-cloud-connector-unknown-id-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssm-amazonssm/src/ops/list_cloud_connectors.rs`. Replaying the corpus, the emulator refused the `ListCloudConnectors` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListCloudConnectors operation: operation ListCloudConnectors is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ssm emulator
Call ListCloudConnectors and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
