Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f45655fc11f6cd82949894c26622ea50244408cc

# CreateSignalingChannel stores ChannelType FULL_MESH although the member doc it quotes says SINGLE_MASTER is the only supported type

_Filed: 2026-11-28T06:10:33Z_
_Operation: CreateSignalingChannel | Class: validation_

> _Note: this issue was identified against the code base at commit `f45655fc11f6cd82949894c26622ea50244408cc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateSignalingChannelInput$ChannelType` is documented as "A type of the signaling channel that you are creating. Currently, SINGLE_MASTER is the only supported channel type." The handler quotes that very sentence in a comment and then applies it only to the DEFAULT: a request that NAMES a type is stored as sent, so `ChannelType: "FULL_MESH"` is answered 200, persisted on the record, reported back by `DescribeSignalingChannel`/`ListSignalingChannels` as `ChannelType: FULL_MESH`, and registered on the signaling data plane as a servable channel.

The modelled enum lists both values, but the member's own documentation states which of them the service supports, and the only type this crate ever mints by itself is `SINGLE_MASTER` (`state::CHANNEL_TYPE_SINGLE_MASTER`, whose doc quotes the same sentence). The operation declares `InvalidArgumentException` ("The value for this input parameter is invalid.") and the handler never constructs it — no input value is refused anywhere in the file — so the unsupported channel type has no refusal path at all and instead becomes durable state. Downstream the stored value leaks into refusal wording (`GetSignalingChannelEndpoint` answers "SingleMasterChannelEndpointConfiguration is required for FULL_MESH channels."), i.e. the crate goes on to describe a SINGLE_MASTER-only configuration requirement for a channel it let the caller declare as mesh.

This is the handler contradicting the sentence printed directly above the line that implements it, not a missing feature: the branch, the constant and the modelled error are all already there.

## Evidence

Model (kind A) — `smithy_query(query="doc", shape="CreateSignalingChannelInput", member="ChannelType")`:
```
<p>A type of the signaling channel that you are creating. Currently,
                <code>SINGLE_MASTER</code> is the only supported channel type. </p>
```

The handler quotes that sentence and then stores whatever was sent — `src/ops/create_signaling_channel.rs:233-237`:
```
            // "Currently, SINGLE_MASTER is the only supported channel type", so that is
            // what a create naming none gets; a named type is stored as sent.
            channel_type: input
                .channel_type
                .unwrap_or_else(|| crate::state::CHANNEL_TYPE_SINGLE_MASTER.to_string()),
```

The crate's own constant states the same rule — `src/state.rs:88-92`:
```
/// The `ChannelType` a `CreateSignalingChannel` call that names none gets: "Currently,
/// `SINGLE_MASTER` is the only supported channel type" (`ChannelType`'s docs on
/// `CreateSignalingChannelInput`), from
/// [`crate::ops::types::CHANNEL_TYPE_VALUES`].
pub const CHANNEL_TYPE_SINGLE_MASTER: &str = "SINGLE_MASTER";
```

The modelled refusal exists and is never used — `src/ops/create_signaling_channel.rs:113` declares `InvalidArgumentException(InvalidArgumentException)` in `CreateSignalingChannelErrorModeled`; the only errors the handler constructs are `ResourceInUseException` (line 191), `AccountChannelLimitExceededException` (lines 204, 292), `ClientLimitExceededException` (line 171) and `CreateSignalingChannelError::not_implemented` (line 308).

Test coverage: `tests/all/signaling_channel_lifecycle.rs:690-697` pins the current acceptance, with the enum membership as its only stated reason — which is what the member doc contradicts:
```
        (
            "the other modeled ChannelType",
            "full-mesh",
            serde_json::json!({ "ChannelType": "FULL_MESH" }),
            200,
            "FULL_MESH",
            60,
        ),
```
and `tests/all/signaling_channel_lifecycle.rs:1843` builds a `FULL_MESH` channel to exercise endpoint refusal wording (`for channel_type in ["SINGLE_MASTER", "FULL_MESH"]`). No test or comment in the crate states a reason for supporting a channel type the member doc says is unsupported.

## Steps to Reproduce

1. POST /createSignalingChannel {"ChannelName":"mesh","ChannelType":"FULL_MESH"} — expected a refusal of the input value (the member doc: "Currently, SINGLE_MASTER is the only supported channel type"); actual HTTP 200 with a ChannelARN
2. POST /describeSignalingChannel {"ChannelName":"mesh"} — ChannelInfo.ChannelType is reported as FULL_MESH, i.e. the unsupported type is now persisted state
3. POST /getSignalingChannelEndpoint {"ChannelARN":<arn>} — the refusal reads "SingleMasterChannelEndpointConfiguration is required for FULL_MESH channels.", a SINGLE_MASTER-only requirement stated for a channel the create accepted as mesh
4. grep InvalidArgumentException src/ops/create_signaling_channel.rs — declared at line 113, never constructed, so no input value of this operation is ever refused by the handler
