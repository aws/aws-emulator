Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: payment-cryptography.create-alias-duplicate-name-conflict-rejected payment-cryptography.delete-key-conflict-when-already-pending-rejected payment-cryptography.key-replication-regions-add-remove-state-transition
Issue-Shape-Id: com.amazonaws.paymentcryptography#PaymentCryptographyControlPlane
Issue-Key: payment-cryptography.create-alias-duplicate-name-conflict-rejected|run#10|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: payment-cryptography.delete-key-conflict-when-already-pending-rejected|run#10|AssertionError|assert code == "ValidationException", (
Issue-Key: payment-cryptography.key-replication-regions-add-remove-state-transition|run#7|KeyError|assert created["MultiRegionKeyType"] == "PRIMARY", (
Issue-Commit: 9002b66bbeb506ed04d96e15ac257c55fd6b9ccf

# payment-cryptography control plane: duplicate-alias and already-pending DeleteKey refusals use emulator wording/class, and a replicated key's CreateKey response omits the Multi-Region members (3 causes)

> _Note: this issue was identified against the code base at commit `9002b66bbeb506ed04d96e15ac257c55fd6b9ccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAlias

## Description
Three independent root causes in the Payment Cryptography control plane, found by behavioral replay. Two are refusal-fidelity defects (one message wording, one wrong error class AND wording) and one is a response-projection defect on CreateKey. Numbered below; each must be closed before this issue is submitted.

Cause 1 of 3 — CreateAlias's duplicate-name ConflictException message is the emulator's own sentence.
Failing check: payment-cryptography.create-alias-duplicate-name-conflict-rejected.
AWS: Smithy — CreateAlias models ConflictException (HTTP 409, single member `Message`: "The requested operation conflicts with the current state of the resource ... trying to create a resource that already exists"); the operation's own documentation states "The alias must be unique in the account and Amazon Web Services Region". Recorded behavior: creating an alias whose name is already bound (to a different key) is refused with Code=ConflictException and the message `Alias <AliasName> already exists.` — fixed text `Alias `, then the AliasName exactly as supplied in the request, then the fixed text ` already exists.` (trailing period included). The only variable part is the AliasName; the name in any one run (e.g. `alias/sim2real-pc-dup-...`) fills that slot and is not part of the template.
Emulator: the error CODE is right (ConflictException) but the message is `the alias <AliasName> already exists and is bound to another key` — a sentence this crate authors itself (services/service-com-amazonaws-paymentcryptography-paymentcryptographycontrolplane/src/ops/create_alias.rs, the ConflictException built in `handle`). Lower-case lead-in, different wording, no trailing period, and an extra clause AWS does not emit.
Why: callers and test suites match on the refusal sentence (log scraping, user-facing surfacing, golden-file assertions). A client that recognises AWS's "Alias X already exists." sees an unrecognised string and reports an unknown failure instead of a name collision.
Fix: the CreateAlias handler's ConflictException construction — emit AWS's fixed sentence with the request's AliasName interpolated. (Note the same handler also chooses to answer the repeat of an IDENTICAL create idempotently; that part is not this cause.) Related but distinct open issue: 20261031T105700Z-payment-cryptography-alias-registry-enforces-neither-uniqueness-nor.md — uniqueness IS now enforced, so that issue does not cover this wording defect.
Traces: check payment-cryptography.create-alias-duplicate-name-conflict-rejected.

Cause 2 of 3 — DeleteKey on a key already DELETE_PENDING is refused with the wrong error class and the wrong message, and a repeat with the same waiting period is not refused at all.
Failing check: payment-cryptography.delete-key-conflict-when-already-pending-rejected.
AWS: Smithy — DeleteKey models both ValidationException (HTTP 400) and ConflictException (HTTP 409); Key.KeyState is required and moves to DELETE_PENDING on the first DeleteKey. Recorded behavior: scheduling deletion on a key that is already DELETE_PENDING is refused with Code=ValidationException and the message `Key <KeyArn> not in CREATE_COMPLETE state.` — fixed text `Key `, then the key's ARN from the request/state, then the fixed text ` not in CREATE_COMPLETE state.` The refusal is unconditional on the requested DeleteKeyInDays: AWS gates on the key's state, not on whether the waiting period matches. The original DeletePendingTimestamp is left untouched.
Emulator: answers Code=ConflictException (HTTP 409) with its own sentence `the key <KeyIdentifier> is already scheduled for deletion in <n> days; restore it before scheduling a different waiting period` (services/.../src/ops/delete_key.rs, the `pending && key.delete_key_in_days != Some(waiting_period)` branch). Worse, when the second DeleteKey names the SAME waiting period the handler treats it as an idempotent retry and answers 200, where AWS still refuses with ValidationException because the key is not in CREATE_COMPLETE state.
Why: the class decides client behavior. A caller that handles ValidationException (a 400 bad-request) sees a 409 ConflictException it does not expect, and retry/backoff logic keyed on the class diverges; an emulator that accepts the same-period repeat with 200 lets code pass that real AWS rejects, which is the overly-permissive direction and hides the bug until production.
Fix: the DeleteKey handler's state gate — refuse ANY DeleteKey against a key whose effective state is not CREATE_COMPLETE with ValidationException and AWS's fixed sentence (ARN interpolated), instead of the ConflictException/idempotent-retry rule the handler currently applies. The emulator's "already deleted" branch should be reviewed with it for the same class/wording rule.
Traces: check payment-cryptography.delete-key-conflict-when-already-pending-rejected.

Cause 3 of 3 — CreateKey with ReplicationRegions answers a Key with no Multi-Region members at all.
Failing check: payment-cryptography.key-replication-regions-add-remove-state-transition.
AWS: Smithy — CreateKeyInput.ReplicationRegions (list of Region) and CreateKeyOutput.Key : Key. On Key, `MultiRegionKeyType` (enum PRIMARY|REPLICA) "Indicates whether this key is a Multi-Region key and its role in the Multi-Region key hierarchy", `ReplicationStatus` is a map Region -> ReplicationStatusType whose required `Status` (KeyReplicationState: IN_PROGRESS | DELETE_IN_PROGRESS | FAILED | SYNCHRONIZED) "indicates whether the key replication is in progress, completed successfully, or has encountered an error", and `UsingDefaultReplicationRegions` is false when "the key has a custom replication configuration that overrides the account defaults". Recorded behavior: a key created with an explicit ReplicationRegions list comes back with MultiRegionKeyType=PRIMARY and a ReplicationStatus entry per requested Region whose Status starts IN_PROGRESS and converges to SYNCHRONIZED; RemoveKeyReplicationRegions then drops that Region's entry entirely. The region names are the request's own inputs, not a requirement; the requirement is one entry per requested Region.
Emulator: CreateKey accepts ReplicationRegions (it is carried into state::NewKey in src/ops/create_key.rs) but the Key projection omits the members — the check's `created["MultiRegionKeyType"]` raises KeyError, i.e. the member is absent from the 200 body, and ReplicationStatus is absent too, so the subsequent poll for SYNCHRONIZED and the post-removal poll can never be satisfied.
Why: a client that creates a Multi-Region key and then waits for its replicas to become usable has nothing to read: it cannot tell a PRIMARY from a single-Region key, cannot observe replication progress, and cannot confirm a region was removed — it waits forever or proceeds against a key it believes is replicated.
Fix: the key record and its Key projection in the control-plane state (src/state.rs: `mint_key` must retain the requested regions and `to_key` must project MultiRegionKeyType, the ReplicationStatus map and UsingDefaultReplicationRegions), so CreateKey, GetKey, DeleteKey and the Add/RemoveKeyReplicationRegions handlers all report them. Open issue 20261031T105700Z-payment-cryptography-key-mutations-never-reach-getkey-usage.md files the same missing members on the GetKey/DeleteKey READ path after AddKeyReplicationRegions; it does not carry this check's replay key and does not cover the CreateKey(ReplicationRegions=...) response path, so it is named here rather than relied on.
Traces: check payment-cryptography.key-replication-regions-add-remove-state-transition.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, payment-cryptography 8/21 passed (13 failed; 10 of the failures are already filed as unimplemented-stub clusters).

Cause 1 — [mismatch] payment-cryptography.create-alias-duplicate-name-conflict-rejected:
  AssertionError: expected message 'Alias alias/sim2real-pc-dup-419f23da6901 already exists.', got 'the alias alias/sim2real-pc-dup-419f23da6901 already exists and is bound to another key'
  Code was ConflictException (correct); only the Message diverges. The alias name shown is the check's generated name — the template is 'Alias <AliasName> already exists.'

Cause 2 — [mismatch] payment-cryptography.delete-key-conflict-when-already-pending-rejected:
  AssertionError: scheduling deletion on a key already DELETE_PENDING must be refused with ValidationException; got 'ConflictException'
  The check's next assertion is the message, expected 'Key <KeyArn> not in CREATE_COMPLETE state.'; the emulator's sentence is 'the key <id> is already scheduled for deletion in <n> days; restore it before scheduling a different waiting period'. The check's first DeleteKey (DeleteKeyInDays=5) correctly reached DELETE_PENDING; the second (DeleteKeyInDays=30) produced the 409. Per src/ops/delete_key.rs the same-waiting-period repeat returns 200 instead of any refusal.

Cause 3 — [error] payment-cryptography.key-replication-regions-add-remove-state-transition:
  KeyError: 'MultiRegionKeyType' at `assert created["MultiRegionKeyType"] == "PRIMARY"`, where `created` is CreateKey(..., ReplicationRegions=[<other region>])['Key'] — a 200 response whose Key omits MultiRegionKeyType, and likewise ReplicationStatus (the check then polls GetKey for ReplicationStatus.<region>.Status == 'SYNCHRONIZED' and, after RemoveKeyReplicationRegions, for the region's absence).
  Smithy cited: Key.MultiRegionKeyType (MultiRegionKeyType: PRIMARY|REPLICA), Key.ReplicationStatus (map Region -> ReplicationStatusType, Status required, KeyReplicationState: IN_PROGRESS|DELETE_IN_PROGRESS|FAILED|SYNCHRONIZED), Key.UsingDefaultReplicationRegions; CreateKeyInput.ReplicationRegions.

## Steps to Reproduce
Cause 1: CreateKey twice (AES_128 symmetric data-encryption key, Exportable=true) to get key1 and key2. CreateAlias(AliasName='alias/<unique>', KeyArn=key1). Call CreateAlias again with the SAME AliasName and KeyArn=key2. Observed: ConflictException with message 'the alias <name> already exists and is bound to another key'. Expected: ConflictException with message 'Alias <name> already exists.'

Cause 2: CreateKey as above. DeleteKey(KeyIdentifier=arn, DeleteKeyInDays=5) -> KeyState DELETE_PENDING. Then DeleteKey(KeyIdentifier=arn, DeleteKeyInDays=30). Observed: ConflictException (409) with 'the key <arn> is already scheduled for deletion in 5 days; restore it before scheduling a different waiting period'. Expected: ValidationException (400) with 'Key <arn> not in CREATE_COMPLETE state.', and the key's DeletePendingTimestamp unchanged. Also call DeleteKey(KeyIdentifier=arn, DeleteKeyInDays=5) a second time: observed 200, expected the same ValidationException.

Cause 3: CreateKey(KeyAttributes=<AES_128 TR31_D0 symmetric>, Exportable=true, ReplicationRegions=['us-east-1']) (any region other than the client's). Observed: 200 whose Key has no MultiRegionKeyType and no ReplicationStatus. Expected: MultiRegionKeyType='PRIMARY' and ReplicationStatus={'us-east-1': {'Status': 'IN_PROGRESS' | 'SYNCHRONIZED'}} converging to SYNCHRONIZED on GetKey, with UsingDefaultReplicationRegions=false. Then RemoveKeyReplicationRegions(KeyIdentifier=arn, ReplicationRegions=['us-east-1']) and GetKey: expected the region's entry to disappear from ReplicationStatus.
