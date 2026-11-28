# Local additions to the Smithy model snapshot

`com-amazonaws-xray-awsxray.json` in this directory is a snapshot of the
published X-Ray model and is kept EXACTLY as shipped: it is a third-party
artefact consumed at build time by `#[smithy_macros::smithy]`, so an edit there
would be invisible to a reader of the crate's source and lost on the next
refresh.

The service admits one value the snapshot does not declare. It is declared in
this crate's own source instead, from the small overlay model
`local-additions.json` beside the snapshot, so the declaration is generated the
same way every modelled shape is — one member list, one set of constraint
traits, no hand-written schema or serializer. The same mechanism is used by
`services/service-com-amazonaws-ecrpublic-spencerfrontendservice`
(`smithy-model/local-additions.json` + `src/ops/types/image_detail_types.rs`)
and by
`services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice`,
whose overlay restates an operation input whose enforced constraint is not the
published one.

## `TraceSegmentDestination$Composite`

- **What AWS does**: `UpdateTraceSegmentDestination` accepts a third trace
  segment destination beside `XRay` and `CloudWatchLogs`, and a request naming a
  value outside the type is refused with the constraint clause
  `Member must satisfy enum value set: [Composite, CloudWatchLogs, XRay]` —
  which is the service's own statement of the values its destination type
  admits. `GetTraceSegmentDestination` reports that destination back like any
  other.
- **What the snapshot declares**: `com.amazonaws.xray#TraceSegmentDestination`
  with `XRay` and `CloudWatchLogs` only, so a request naming the third
  destination would be refused as a value outside the type and the refusal of
  any other value would state a set short of what the service admits.
- **Where it is declared**: `src/ops/types/update_trace_segment_destination_types.rs`
  (the input and output of `UpdateTraceSegmentDestination`, so the destination a
  request may carry and the one the answer reports are the same set) and
  `src/ops/types/get_trace_segment_destination_types.rs` (the output of the
  read). The value catalogue those two operations validate and report against is
  `ADMITTED_TRACE_SEGMENT_DESTINATIONS`, beside the first of them. The ORDER a
  refusal states the set in is not a property of the model and lives where every
  other value-set order of this service does, in `src/service.rs`.
- **What else is in the overlay**: only the shapes those members target, and
  only as much of each as codegen reads — the other enum its type and values,
  the nested structure its member, and that member's target string its length
  bound, all restated exactly as the snapshot spells them.

`tests/all/trace_segment_destination_local_addition.rs` pins both halves: the
overlay against the snapshot, declaration for declaration, so a restatement that
drops a member, a bound or a value fails; and the wire, where every admitted
destination is configured and reported back and a value outside the set is
refused stating the whole set. If the snapshot is ever refreshed with the third
destination, that suite fails — and the local declaration should then give way
to the modelled shape.
