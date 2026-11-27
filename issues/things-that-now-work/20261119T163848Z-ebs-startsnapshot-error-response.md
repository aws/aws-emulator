Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 23a6305fb4194ac652f562ce813f73dafa581013

# A StartSnapshot retry that spells the default Timeout explicitly is refused 409 ConflictException instead of replaying

_Filed: 2026-11-19T16:38:48Z_
_Operation: StartSnapshot | Class: error-response_

> _Note: this issue was identified against the code base at commit `23a6305fb4194ac652f562ce813f73dafa581013`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartSnapshot`'s `ClientToken` fingerprint canonicalises an absent `Timeout` as `0`, while the handler itself canonicalises an absent `Timeout` as the documented default of 60 minutes when it builds the snapshot. The two disagree, so two requests that create the identical snapshot produce different fingerprints:

* original: `{"VolumeSize": 1, "ClientToken": "t"}` — stored with `timeout_minutes = 60` and fingerprinted over `Timeout=0`;
* retry: `{"VolumeSize": 1, "Timeout": 60, "ClientToken": "t"}` — fingerprinted over `Timeout=60`.

The retry asks for exactly the snapshot the store already holds (same volume size, same parent, same tags, same expiry), yet it is answered with the modelled `ConflictException` (HTTP 409, "Current request does not match request stored with this token.") instead of replaying the original 201 answer. `Timeout` is the only member whose canonical form in the fingerprint departs from the value the handler applies: the sibling line for `Encrypted` in the SAME function canonicalises with `unwrap_or(false)`, which is exactly how the handler reads that member, so absent and explicit `false` correctly collide. Because `Timeout`'s modelled `@range` is 10..4320, no legal request can carry `0`, so the stray sentinel can only produce false conflicts — it never collides two genuinely different requests.

## Evidence

The default the handler APPLIES — services/service-com-amazonaws-ebs-ebs/src/ops/start_snapshot.rs:401:
"        let timeout_minutes = input.timeout.unwrap_or(DEFAULT_SNAPSHOT_TIMEOUT_MINUTES);"
stated once in the state module — services/service-com-amazonaws-ebs-ebs/src/state.rs:24-26:
"/// `StartSnapshotRequest$Timeout` documents: \"If no value is specified, the timeout
/// defaults to 60 minutes.\"
pub const DEFAULT_SNAPSHOT_TIMEOUT_MINUTES: i32 = 60;"

The default the FINGERPRINT applies — services/service-com-amazonaws-ebs-ebs/src/ops/start_snapshot.rs:521:
"    canonical.push_str(&format!(\"Timeout={}\\n\", input.timeout.unwrap_or_default()));"
against the sibling member two lines above it, which uses the value the handler reads (start_snapshot.rs:513-516):
"    canonical.push_str(&format!(
        \"Encrypted={}\\n\",
        input.encrypted.unwrap_or(false)
    ));"
(the handler reads `Encrypted` exactly that way at start_snapshot.rs:391: "let encrypted = input.encrypted == Some(true) || input.kms_key_arn.is_some();").

What the fingerprint is FOR, per its own doc — start_snapshot.rs:500-502:
"/// A stable digest of the request members a `ClientToken` is scoped to, so a retry of
/// the identical request is recognised and a different one is a `ConflictException`."
and the refusal it drives — start_snapshot.rs:298-307 (`if record.request_fingerprint != fingerprint { return Err(...ConflictException...) }`).

The modelled contract this breaks — services/service-com-amazonaws-ebs-ebs/src/ops/start_snapshot.rs:55-62 (`ClientToken`): "The subsequent retries with the same client token return the result from the original successful request and they have no additional effect." and start_snapshot.rs:130-131 (`ConflictException`): "The request uses the same client token as a previous, but non-identical request."

Test coverage: the conflict table covers a changed VolumeSize, Description, Tags and added encryption only — services/service-com-amazonaws-ebs-ebs/tests/all/response_bodies_and_refusals.rs:579-612 — and the replay table replays byte-identical bodies — services/service-com-amazonaws-ebs-ebs/tests/all/snapshot_block_loop.rs:1086-1131 (the "encrypted with a timeout" row sends `"Timeout": 30` in BOTH calls). No test sends a retry that differs only by spelling the default Timeout, so canonicalising it with DEFAULT_SNAPSHOT_TIMEOUT_MINUTES leaves every existing test green.

## Steps to Reproduce

1. POST /snapshots with {"VolumeSize": 1, "ClientToken": "t"} — HTTP 201; the stored snapshot gets timeout_minutes = 60 (the documented default).
2. POST /snapshots again with {"VolumeSize": 1, "Timeout": 60, "ClientToken": "t"} — the same snapshot is being asked for, with the default spelled out.
3. Observe HTTP 409 ConflictException with Message "Current request does not match request stored with this token." instead of a replay of the original 201 body.
4. Control: POST /snapshots with {"VolumeSize": 1, "Timeout": 60, "ClientToken": "u"} then repeat it verbatim — HTTP 201 twice with the same SnapshotId, showing the refusal above comes only from the absent-vs-explicit default and not from the value.
