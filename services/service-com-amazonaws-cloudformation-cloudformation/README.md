# service-com-amazonaws-cloudformation-cloudformation

Emulator implementation for the `com-amazonaws-cloudformation-cloudformation` service. This crate is
self-contained: building it does NOT rebuild the other services.

```bash
cargo build -p service-com-amazonaws-cloudformation-cloudformation
cargo test  -p service-com-amazonaws-cloudformation-cloudformation
```

## What is implemented today

The stack lifecycle, computed from the stack store in `src/state.rs`:
`CreateStack`, `DescribeStacks`, `ListStacks`, `GetTemplate`, `UpdateStack`,
`DeleteStack` and `DescribeStackEvents`. A create/update/delete answers while the
operation is in progress and the stack settles to its terminal status a moment later,
so a caller that reads straight after the call still sees `*_IN_PROGRESS`
(`state::STATUS_TRANSITION_DELAY`). `tests/all/stack_lifecycle.rs` drives all of it over
the awsQuery wire.

A create or update whose template declares an `AWS::CloudFormation::WaitCondition` waits
for the success signals its `Count` asks for (`SignalResource` delivers them) rather than
for that clock. It is held for the `Timeout` the declaration states — the span the caller
asked for, bounded by the maximum that property publishes (twelve hours) and defaulted to
the documented `PT5M` where it states none — so a resource nothing has signalled stays
`*_IN_PROGRESS`, and reportable, for the whole of that span and the operation then fails
at its deadline. A failed
update is then rolled back to the previous stack configuration in the services that own
its resources, which is what `UPDATE_ROLLBACK_COMPLETE` / `UPDATE_ROLLBACK_FAILED` and
`ContinueUpdateRollback` report on (`tests/all/update_rollback_recovery.rs`).

Every other operation (change sets, stack sets, the type registry, drift detection,
resource scans, refactors, imports/exports) is still scaffolded and answers HTTP 501.

## Layout

```
service-com-amazonaws-cloudformation-cloudformation/
├── build.rs              # calls emulator_core_build_support::generate(...)
├── smithy-model/
│   ├── com-amazonaws-cloudformation-cloudformation.json      # the Smithy model — drives routing + wire shapes
│   ├── local-additions.json  # overlay model for the elements AWS answers with that the snapshot lacks
│   └── LOCAL-ADDITIONS.md    # what is in that overlay, and why
├── OPERATIONS.md         # every operation's contract, re-rendered from the model
├── tests/
│   └── state_roundtrip.rs  # compiled example of the state API
└── src/
    ├── lib.rs            # #[smithy_service] decl (generated — do not hand-edit)
    ├── service.rs        # impl Service — can_handle + routing/refusal overrides (yours to edit)
    ├── state.rs          # ComAmazonawsCloudformationCloudformationState — this service's in-memory state
    ├── scaffolded.rs     # Scaffolded<E> — the 501 stubs return, and `validation` for a 400
    └── ops/              # ONE FILE PER OPERATION
```

## Implementing an operation

Each file in `src/ops/` owns exactly one operation and declares which one
via an `@op` header comment. `build.rs` globs this directory and generates
both the module list and the operation set, so adding a file is all that
is needed — there is no central registry to update.

Every generated stub starts out **scaffolded, not implemented**: it is fully typed
and registered, and it answers **HTTP 501 `NotImplemented`** until you write it. It
deliberately does not return an empty success — a hollow `200` with a zeroed body is
indistinguishable from a real implementation to a client, and to any check that only
asserts the call succeeded.

Each operation is a struct that owns the shared engine (constructor-injected) and
implements `smithy_server_runtime::Operation`. The framework constructs one per
service via `OpName::new(engine)` and drives it through the centralized request walk:
deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.

To implement an operation, replace the `Err(..)` in `handle` with real behaviour and
delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line at the top of the file. That
marker is how the still-stubbed operations are counted:

```sh
grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' */src/ops/
```

```rust
//! @op CreateThing

impl<Eng: Send + Sync + 'static> smithy_server_runtime::Operation for CreateThing<Eng> {
    type Input = CreateThingInput;
    type Output = CreateThingOutput;
    type Error = CreateThingError;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Scaffolded: `Err(crate::scaffolded::Scaffolded::not_implemented("CreateThing"))`.
        // Modeled failures: `Err(CreateThingErrorModeled::Variant(..).into())`.
        /* ... */
    }
}
```

## State

A handler reaches this service's store through the engine held on `self`
(constructor-injected). The framework builds ONE shared engine and hands it to every
operation of this crate through its `new(engine)` constructor, so every handler in
this crate sees the same store:

```rust
async fn handle(
    &self,
    input: Self::Input,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> Result<Self::Output, Self::Error> {
    let Some(state) = crate::state::service_state(&*self.engine) else {
        return Err(/* mounted on an engine that holds no state: a modelled server error */);
    };
    // A LOCKABLE handle, not the value — read and mutate through the guard:
    state.lock().your_field = /* ... */;
}
```

> **Never write `engine::InMemoryEngine::new()` inside a handler.** `engine` is the
> name of a CRATE as well as of the field, so that line compiles — and silently
> gives the request a private, empty store that is dropped when it returns. The write
> looks like it succeeded and the next call sees nothing. Reach the shared store
> through `self.engine`, never a fresh engine.

State lives in the SINGLE shared engine, keyed by type. Reach it through
`crate::state::service_state`, which `src/state.rs` generates.

Do not add a second engine crate — the router and every service must share
one `InMemoryEngine` type or the engine the router injects cannot be handed to this
service's operations.
