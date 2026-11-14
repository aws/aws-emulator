Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.dataset-and-version-lifecycle-transitions bedrock-agentcore-control.update-dataset-concurrent-version-conflict
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|UpdateDataset|NotImplemented
Issue-Key: bedrock-agentcore-control.update-dataset-concurrent-version-conflict|run#12|AssertionError|assert code == "ConflictException", f"undocumented refusal in the race: {code!r} ({message!r})"
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# Implement bedrock-agentcore-control UpdateDataset: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDataset

## Description
`UpdateDataset` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/update_dataset.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `UpdateDataset` call, so each one unblocks only when `UpdateDataset` works: `bedrock-agentcore-control.dataset-and-version-lifecycle-transitions`, `bedrock-agentcore-control.update-dataset-concurrent-version-conflict`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/update_dataset.rs`. Replaying the corpus, the emulator refused the `UpdateDataset` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateDataset operation: operation UpdateDataset is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the bedrock-agentcore-control emulator
Call UpdateDataset and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
