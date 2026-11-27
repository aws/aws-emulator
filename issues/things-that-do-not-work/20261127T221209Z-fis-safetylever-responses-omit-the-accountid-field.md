Issue-Source: trace-sweep-divergence
Issue-Trace: fis-get-safety-lever-not-found-error
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fis-get-safety-lever-not-found-error fis-safety-lever-baseline-get fis-safety-lever-get-and-noop-update fis-safety-lever-not-found-errors fis-safety-lever-status-enum-validation-error fis-update-safety-lever-not-found-error fis-update-safety-lever-validation-error
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis|GetSafetyLever|2xx->2xx|$.safetyLever.accountId
Issue-Key: fis|UpdateSafetyLeverState|2xx->2xx|$.safetyLever.accountId
Issue-Commit: 0f328bb6ae93185eda2a76c9c7fc6646652b63ca

# fis: SafetyLever responses omit the accountId field

> _Note: this issue was identified against the code base at commit `0f328bb6ae93185eda2a76c9c7fc6646652b63ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSafetyLever

## Description
The emulator's shared SafetyLever serializer emits only id/arn/state, so every operation that returns a safetyLever structure drops the accountId field AWS populates. 2 causes.

Cause 1 of 2: GetSafetyLever omits safetyLever.accountId (fixture fis-get-safety-lever-not-found-error, step task#2).
AWS: recorded behavior only — the on-disk Smithy model's SafetyLever shape (com.amazonaws.fis#SafetyLever) declares only id, arn and state, so accountId is not modeled; the recorded GetSafetyLever 200 response nevertheless carries safetyLever.accountId holding the 12-digit AWS account ID that owns the lever (the same account that appears in the lever's arn). Smithy: GetSafetyLever GET /safetyLevers/{id}, output GetSafetyLeverResponse.safetyLever -> SafetyLever, success code 200.
Emulator: returns HTTP 200 with a safetyLever object whose accountId is absent (replay detail: "$.safetyLever.accountId: missing"). Status, id, arn and state all match; only the member is dropped.
Why: a client that reads safetyLever.accountId to decide which account a global safety lever belongs to (for example to confirm it is operating on its own account's lever before flipping it to Disabled) gets a null/absent value and either crashes on deserialization of a required local type or silently treats the lever as unowned. Cross-account guard logic keyed on that field cannot be exercised against the emulator at all.
Fix: the SafetyLever response serializer (and the lever record in FIS state, if accountId is not stored there) — populate accountId from the account the lever's arn is minted under so every operation returning the structure carries it.
Traces: fis-get-safety-lever-not-found-error#task#2 (representative); cluster also covers fis-safety-lever-baseline-get#task#1, fis-safety-lever-get-and-noop-update#task#1, fis-safety-lever-not-found-errors#task#3, fis-safety-lever-status-enum-validation-error#task#2, fis-update-safety-lever-not-found-error#task#1, fis-update-safety-lever-not-found-error#task#3, fis-update-safety-lever-validation-error#task#1, fis-update-safety-lever-validation-error#task#3.

Cause 2 of 2: UpdateSafetyLeverState omits safetyLever.accountId (fixture fis-safety-lever-get-and-noop-update, step task#2).
AWS: recorded behavior only, same as cause 1 — accountId is absent from the modeled SafetyLever shape but present in the recorded 200 response. Smithy: UpdateSafetyLeverState PATCH /safetyLevers/{id}/state, output UpdateSafetyLeverStateResponse.safetyLever -> SafetyLever, success code 200, errors ConflictException(409)/ResourceNotFoundException(404)/ValidationException(400).
Emulator: returns HTTP 200 after the state update with safetyLever.accountId missing (replay detail: "$.safetyLever.accountId: missing"), including on a no-op update that leaves the state unchanged.
Why: a client that round-trips the updated lever — reading accountId back from the mutation response rather than re-getting the lever — loses the owning account on the write path too, so the gap is not avoidable by calling GetSafetyLever instead.
Fix: same component as cause 1 — the shared SafetyLever serializer used by the UpdateSafetyLeverState handler; fixing it once must cover both read and write paths.
Traces: fis-safety-lever-get-and-noop-update#task#2.

Note: no existing open fis issue covers this; the open issue 20261125T030729Z-fis-listactions-returns-paraphrased-built-in-action-descriptions.md is about ListActions catalog text and is unrelated.

## Evidence
GetSafetyLever (fis-get-safety-lever-not-found-error#task#2): expected status 200, actual 200; $.safetyLever.accountId: missing (expected '000000000000', got None). 9 further steps in the same cluster show the identical omission (fis-safety-lever-baseline-get#task#1, fis-safety-lever-not-found-errors#task#3, fis-update-safety-lever-validation-error#task#1, ...).
UpdateSafetyLeverState (fis-safety-lever-get-and-noop-update#task#2): expected status 200, actual 200; $.safetyLever.accountId: missing (expected '000000000000', got None).
In both cases every other member of the returned safetyLever matched; the sole divergence is the absent accountId member. The recorded value is the replay account's own 12-digit id and is environment-specific — the requirement is that the field is present and names the account owning the lever, not the literal digits.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus).
2. Cause 1: replay fixture fis-get-safety-lever-not-found-error step task#2 — GET /safetyLevers/{id} for an existing lever (e.g. the AWS-FIS-Account-S3-Pause-Replication style lever the baseline fixture seeds). Inspect the JSON body: safetyLever has id, arn and state but no accountId key.
3. Cause 2: replay fixture fis-safety-lever-get-and-noop-update step task#2 — PATCH /safetyLevers/{id}/state with the lever's current state (a no-op update). The 200 body's safetyLever again has no accountId key.
4. Compare against the recorded responses, which carry safetyLever.accountId set to the owning account id.
