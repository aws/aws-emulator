# Local additions to the Smithy model snapshot

`com-amazonaws-inspector2-inspector2.json` in this directory is a snapshot of the
published Amazon Inspector (inspector2) model and is kept EXACTLY as shipped: it
is a third-party artefact consumed at build time by `#[smithy_macros::smithy]`,
so an edit there would be invisible to a reader of the crate's source and lost on
the next refresh.

AWS answers one of this service's reads with a member set NARROWER than that
snapshot declares: two members the snapshot marks `@required` are absent from the
answer. That shape is declared in this crate's own source instead, from a small
overlay model beside the snapshot (`local-additions.json`), so the declaration is
generated the same way every modelled shape is — one member list, one set of
constraint traits, no hand-written schema or serializer. The same mechanism is
used by `services/service-com-amazonaws-codestarconnections-codestar-connections-20191201`
(`smithy-model/local-additions.json` +
`src/ops/types/get_resource_sync_status_types/overlay.rs`) and by
`services/service-com-amazonaws-arczonalshift-percdataplane`.

## `State.errorCode` and `State.errorMessage` for a scan that did not fail

- **What AWS does**: `BatchGetAccountStatus` reports an account's Amazon
  Inspector posture as a `State` per scan type and one for the account itself,
  each carrying the `status` in force. `errorCode` and `errorMessage` explain why
  an account FAILED to enable Amazon Inspector, and `ErrorCode` admits no token
  for "nothing failed" (`smithy_query(query="shape", shape="ErrorCode")`), so a
  posture with no failure behind it carries neither member: an account the caller
  may not read is reported per entry in `failedAccounts`, where both members are
  genuinely required, and never as a `State` with an empty error code.
- **What the snapshot declares**: `State` with `status` (target `Status`),
  `errorCode` (target `ErrorCode`) and `errorMessage` (target `NonEmptyString`)
  all marked `@required`.
- **Where it is declared**: `src/ops/types/batch_get_account_status_types/overlay.rs`,
  generated from `local-additions.json`, which restates the shape with those two
  members optional and against the snapshot's own member targets. What `status`
  carries is the enablement this service holds for the account
  (`crate::state::AccountEnablement`).

## What else is in the overlay

Only the shapes those members target, and only as much of each as codegen reads:
`Status`, `ErrorCode` and `NonEmptyString` are reached here as the targets of
string members, whose values this service's own state supplies, so the overlay
declares them as the strings they are and restates no constraint of either. The
structures that CARRY a `State` — `AccountState` and `ResourceState` — stay
generated from the snapshot, in
`src/ops/types/batch_get_account_status_types/snapshot.rs`, where codegen takes
each member's shape from the member's own Rust type.

The two models are named from two submodules of
`src/ops/types/batch_get_account_status_types.rs` rather than from one file
because `smithy_macros::smithy_model!` defines the local `smithy!` macro that
`#[smithy]` expands into, and that definition covers the whole file it appears
in — nested modules included.

If the snapshot is ever refreshed without those two `@required` traits, the local
declaration should give way to the modelled shape.
