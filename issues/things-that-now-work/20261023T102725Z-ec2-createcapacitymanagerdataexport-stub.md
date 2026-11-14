Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-capacity-manager-data-export-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-capacity-manager-data-export-lifecycle ec2-dry-run-validation-before-commit
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:CreateCapacityManagerDataExport
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 CreateCapacityManagerDataExport: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityManagerDataExport

## Description
`CreateCapacityManagerDataExport` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_capacity_manager_data_export.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `CreateCapacityManagerDataExport` call, so each one unblocks only when `CreateCapacityManagerDataExport` works: `ec2-capacity-manager-data-export-lifecycle#task#2`, `ec2-capacity-manager-data-export-lifecycle#task#3`, `ec2-dry-run-validation-before-commit#task#3`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_capacity_manager_data_export.rs`. Replaying the corpus, the emulator refused the `CreateCapacityManagerDataExport` call: status: expected 412, got 501
$.Response.Errors.Error.Code: value_mismatch (expected 'DryRunOperation', got 'NotImplemented')
$.Response.Errors.Error.Message: value_mismatch (expected 'Request would have succeeded, but DryRun flag is set.', got 'operation CreateCapacityManagerDataExport is scaffolded but not implemented in this emulator')
$.Response.RequestID: missing (expected 'ed0d14d5-b605-4c00-8704-fc8914cecf00', got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call CreateCapacityManagerDataExport and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
