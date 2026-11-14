Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-dhcp-options-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-dhcp-options-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DeleteDhcpOptions
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 DeleteDhcpOptions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDhcpOptions

## Description
`DeleteDhcpOptions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_dhcp_options.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteDhcpOptions` call, so each one unblocks only when `DeleteDhcpOptions` works: `ec2-dhcp-options-lifecycle#task#6`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_dhcp_options.rs`. Replaying the corpus, the emulator refused the `DeleteDhcpOptions` call: status: expected 400, got 501
$.Response.Errors.Error.Code: value_mismatch (expected 'InvalidDhcpOptionsId.Malformed', got 'NotImplemented')
$.Response.Errors.Error.Message: value_mismatch (expected "The dhcp-options ID 'dopt-0123456789abcdef0' is malformed", got 'operation DeleteDhcpOptions is scaffolded but not implemented in this emulator')
$.Response.RequestID: missing (expected '<shaped-not-asserted>', got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteDhcpOptions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
