Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 35f7de95e926ecd075d4f0c5efe73729faf5e0c1

# The stream and channel indexes are not keyed by the region their records' ARNs are minted in, contradicting the crate's own "per-account, per-region identifier" rule

_Filed: 2026-11-28T00:31:53Z_
_Operation: CreateStream | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `35f7de95e926ecd075d4f0c5efe73729faf5e0c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every record in this store carries a region in its ARN but is indexed by bare name, so the region a request is scoped to is written into state and then never used to find it again.

`CreateStream` reads the request's region from its SigV4 credential scope (`crate::state::request_region`) and mints `stream_arn(&region, …)`, but its uniqueness check and the index it inserts into are keyed on `input.stream_name` alone — while the comment directly above that check states the rule it is supposed to implement: "The stream name is the per-account, per-region identifier". The same mismatch holds for `CreateSignalingChannel` and the channel index, whose field doc states the rule too, and for `LookupError::NotFound`, documented as "No resource of that name or ARN exists in this account and region".

Two observable consequences, neither of which any comment or test in the crate justifies:
- a create of the same name in a second region is refused `ResourceInUseException` ("The stream <name> already exists.") although, by the quoted rule, that name is free in that region;
- every reader resolves a name across regions: `DescribeStream`/`ListStreams` called with a eu-west-2 credential scope returns the record created under us-east-1, ARN region field and all, so the response names a resource of a region the caller never addressed.

Nothing else in the crate scopes by region: `request_region` is called in exactly two places (`create_stream.rs:201`, `create_signaling_channel.rs:182`), both only to build an ARN or the default KMS key ARN, and no index key, `resolve`, `resolve_channel`, `resource_tags` or page token carries a region.

## Evidence

The handler comment stating the rule, and the code under it that ignores the region — `src/ops/create_stream.rs:201-215`:
```
        let region = crate::state::request_region(ctx.parts());
...
        // The stream name is the per-account, per-region identifier, so a create
        // under a name already in the store is a conflict, never an update.
        if store.streams.contains_key(&input.stream_name) {
            return Err(CreateStreamErrorModeled::ResourceInUseException(
```
and the region that IS used, only for the ARN — `src/ops/create_stream.rs:237-238`:
```
        let stream_arn =
            crate::state::stream_arn(&region, &input.stream_name, creation_time_millis);
```
with `src/state.rs:823-828` writing it into the ARN's region field (`::engine::emulator_arn("kinesisvideo", region, …)`).

The index that is not region-keyed — `src/state.rs:522-528`:
```
    /// The primary stream index. `BTreeMap`, not `HashMap`: `ListStreams`
    /// paginates over the sorted key set, and a token resolves against that set
    /// rather than against a snapshot copy.
    pub streams: BTreeMap<String, Stream>,
    /// `StreamARN` -> `StreamName`, so every operation that accepts an ARN
    /// resolves to the same record the name form does.
    pub stream_arn_index: BTreeMap<String, String>,
```

The same rule stated for channels, with the same name-only key — `src/state.rs:529-534`:
```
    /// The primary signaling-channel index, keyed by `ChannelName` — the
    /// per-account, per-region identifier a channel is unique under.
```
and for lookups — `src/state.rs:454-455`:
```
    /// No resource of that name or ARN exists in this account and region.
    NotFound,
```
and for the ceilings — `src/state.rs:128` ("The most streams this emulator holds at once, per account and region.") and `src/state.rs:154`.

The framework does not partition it either: `engine::InMemoryEngine::state` keys state by `TypeId` alone (`vendor/engine/src/lib.rs:139-148`), so region scoping is this crate's to apply, and `crate::state::service_state` (`src/state.rs:751-756`) hands every request the one store.

Test coverage: `tests/all/stream_lifecycle.rs:453` documents the rule for one region only ("A stream name is unique per account and region, so a second create under a name …", test fn `a_duplicate_stream_name_is_refused_as_resource_in_use`) and `tests/all/stream_lifecycle.rs:161` asserts the ARN's region field is the credential scope's. The multi-region suites use DISTINCT names per region (`tests/all/signaling_channel_data_plane.rs:164-183`) or a fresh harness per control plane (`tests/all/stream_data_endpoint.rs:347-351`), so no test pins a cross-region collision or a cross-region read.

## Steps to Reproduce

1. POST /createStream {"StreamName":"cam"} with a SigV4 credential scope of us-east-1 — 200, ARN arn:aws:kinesisvideo:us-east-1:…:stream/cam/<millis>
2. POST /createStream {"StreamName":"cam"} with a credential scope of eu-west-2 — expected a create of a distinct per-region resource; actual 400 ResourceInUseException "The stream cam already exists."
3. POST /describeStream {"StreamName":"cam"} with the eu-west-2 scope — 200, returning the us-east-1 record (StreamARN's region field reads us-east-1)
4. POST /listStreams {} with the eu-west-2 scope — the us-east-1 stream is listed, although `LookupError::NotFound` is documented as "No resource of that name or ARN exists in this account and region"
