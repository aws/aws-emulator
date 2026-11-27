Issue-Source: trace-sweep-divergence
Issue-Trace: dsdata-write-and-membership-operation-constraint-envelopes
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dsdata-write-and-membership-operation-constraint-envelopes
Issue-Shape-Id: com.amazonaws.directoryservicedata#DirectoryServiceData
Issue-Key: ds-data|UpdateGroup|4xx->4xx|$.Message
Issue-Key: ds-data|UpdateUser|4xx->4xx|$.Message
Issue-Commit: ba97c8568f94e8f0e1657393f851b27fdc51a8e0

# ds-data ValidationException enumerates the allowed enum value set in the wrong order

> _Note: this issue was identified against the code base at commit `ba97c8568f94e8f0e1657393f851b27fdc51a8e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateGroup

## Description
2 net-new causes, one shared root cause: when a Directory Service Data write operation is given a value outside a modeled enum, the emulator returns the right HTTP status (400) and the right error shape (ValidationException), but the `Message` it serializes lists the permitted enum values in a DIFFERENT ORDER than AWS does. The emulator enumerates them in Smithy model declaration order; AWS enumerates them in its own service-internal order, which is not the model's. The bracketed value set is fixed text for a given enum member (it is derived from the enum, not from the request), so the order is part of the required message, not a per-run value.

Cause 1 of 2: UpdateGroup, member `GroupScope` (enum shape GroupScope).
  AWS: Smithy: UpdateGroup.GroupScope targets com.amazonaws.directoryservicedata#GroupScope; UpdateGroup models error ValidationException (client, httpError 400). Recorded response: HTTP 400, ValidationException, Message = "1 validation error detected: Value 'Regional' at 'groupScope' failed to satisfy constraint: Member must satisfy enum value set: [Global, DomainLocal, BuiltinLocal, Universal]". Fixed text of this message class: "1 validation error detected: Value '" + <rejected value> + "' at '" + <lowerCamel member name> + "' failed to satisfy constraint: Member must satisfy enum value set: [" + <allowed values, in AWS's order> + "]". Variable parts taken from the request: the rejected value and the member name. The allowed-value list and its ORDER are fixed per enum — for GroupScope AWS emits Global, DomainLocal, BuiltinLocal, Universal. Note this is NOT the model's declaration order ([DomainLocal, Global, Universal, BuiltinLocal]) and NOT alphabetical. Hypothesis: AWS serializes its internal enum declaration order, which differs from the published Smithy model's.
  Emulator: HTTP 400, ValidationException, Message = "1 validation error detected: Value 'Regional' at 'groupScope' failed to satisfy constraint: Member must satisfy enum value set: [DomainLocal, Global, Universal, BuiltinLocal]" — i.e. the four values in Smithy declaration order.
  Fix: the enum-constraint branch of UpdateGroup's input validation / the shared ValidationException message formatter for ds-data must emit GroupScope's allowed set in AWS's order rather than iterating the generated enum in model order.
  Traces: dsdata-write-and-membership-operation-constraint-envelopes#task#1.

Cause 2 of 2: UpdateUser, member `UpdateType` (enum shape UpdateType).
  AWS: Smithy: UpdateUser.UpdateType targets com.amazonaws.directoryservicedata#UpdateType; UpdateUser models error ValidationException (client, httpError 400). Recorded response: HTTP 400, ValidationException, Message = "1 validation error detected: Value 'MERGE' at 'updateType' failed to satisfy constraint: Member must satisfy enum value set: [ADD, REMOVE, REPLACE]". Same message template as cause 1; the rejected value ('MERGE') and member name ('updateType') come from the request, the bracketed set is fixed per enum. AWS's order for UpdateType is ADD, REMOVE, REPLACE; the model declares ADD, REPLACE, REMOVE.
  Emulator: HTTP 400, ValidationException, Message = "1 validation error detected: Value 'MERGE' at 'updateType' failed to satisfy constraint: Member must satisfy enum value set: [ADD, REPLACE, REMOVE]".
  Fix: same component — the enum-constraint message formatter used by UpdateUser's input validation; UpdateType is shared by UpdateGroup and UpdateUser, so fixing the ordering source fixes both call sites.
  Traces: dsdata-write-and-membership-operation-constraint-envelopes#task#2.

Why: clients and test suites assert on ValidationException message text for enum rejections — it is the only place the API tells a caller which values are legal. Anything that string-matches or snapshot-compares the message (SDK integration tests, CLI output assertions, contract tests, error-parsing middleware that extracts the permitted set to retry with a legal value) sees a message that AWS never emits, so a caller that passes against real AWS fails against the emulator (and vice versa) purely on value ordering. Because the status and error code are correct, the failure surfaces only as a confusing text diff rather than an obvious wiring bug.

No open issue covers this: the two open ds-data issues (20261127T095643Z-ds-data-every-managed-ad-data-plane-rejection.md and 20261022T085720Z-ds-ds-data-cross-plane-contract-broken-ds.md) concern directory Stage settling and cross-plane ResourceNotFound/AccessDenied classification respectively, not enum-constraint message formatting.

## Evidence
Replay of fixture dsdata-write-and-membership-operation-constraint-envelopes (trace set simparity-wire-aws-v1), 26 steps, 24 passed, 2 mismatched.

Cause 1 — step task#1, UpdateGroup, expected_status 400, actual_status 400, verdict mismatch, divergence key service=ds-data|UpdateGroup|status=400->400|keys=$.Message:
  $.Message expected: "1 validation error detected: Value 'Regional' at 'groupScope' failed to satisfy constraint: Member must satisfy enum value set: [Global, DomainLocal, BuiltinLocal, Universal]"
  $.Message actual:   "1 validation error detected: Value 'Regional' at 'groupScope' failed to satisfy constraint: Member must satisfy enum value set: [DomainLocal, Global, Universal, BuiltinLocal]"

Cause 2 — step task#2, UpdateUser, expected_status 400, actual_status 400, verdict mismatch, divergence key service=ds-data|UpdateUser|status=400->400|keys=$.Message:
  $.Message expected: "1 validation error detected: Value 'MERGE' at 'updateType' failed to satisfy constraint: Member must satisfy enum value set: [ADD, REMOVE, REPLACE]"
  $.Message actual:   "1 validation error detected: Value 'MERGE' at 'updateType' failed to satisfy constraint: Member must satisfy enum value set: [ADD, REPLACE, REMOVE]"

Model corroboration (smithy_query, services/service-com-amazonaws-directoryservicedata-directoryservicedata/smithy-model/com-amazonaws-directoryservicedata-directoryservicedata.json):
  GroupScope values = [DomainLocal, Global, Universal, BuiltinLocal]  <- matches the emulator's actual output exactly
  UpdateType values = [ADD, REPLACE, REMOVE]                          <- matches the emulator's actual output exactly
This confirms the emulator is iterating the Smithy enum in declaration order while AWS emits a different, service-internal order. Everything else about both responses (status 400, ValidationException shape, the fixed text, the rejected value and the lowerCamel member name) already matches, so the ordering of the bracketed set is the sole divergence.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay fixture `dsdata-write-and-membership-operation-constraint-envelopes` from trace set simparity-wire-aws-v1.
3. Cause 1 (step task#1): POST /Groups/UpdateGroup?DirectoryId=<a d-XXXXXXXXXX directory> with SAMAccountName set to an existing group and GroupScope set to a value outside the enum (the fixture uses 'Regional'). Observe HTTP 400 ValidationException and compare $.Message to the recorded one: the bracketed enum value set comes back as [DomainLocal, Global, Universal, BuiltinLocal] instead of AWS's [Global, DomainLocal, BuiltinLocal, Universal].
4. Cause 2 (step task#2): POST /Users/UpdateUser?DirectoryId=<same directory> with SAMAccountName set to an existing user and UpdateType set to a value outside the enum (the fixture uses 'MERGE'). Observe HTTP 400 ValidationException and compare $.Message: the set comes back as [ADD, REPLACE, REMOVE] instead of AWS's [ADD, REMOVE, REPLACE].
5. Both causes must be closed before submitting; re-replay the fixture and confirm steps task#1 and task#2 both pass.
