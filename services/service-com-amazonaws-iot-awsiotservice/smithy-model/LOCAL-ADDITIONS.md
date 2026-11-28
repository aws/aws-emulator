# Local additions to the Smithy model snapshot

`com-amazonaws-iot-awsiotservice.json` in this directory is a snapshot of the published AWS
IoT model and is kept EXACTLY as shipped: it is a third-party artefact consumed at build
time by `#[smithy_macros::smithy]`, so an edit there would be invisible to a reader of the
crate's source and lost on the next refresh of the snapshot.

One ENUMERATED TYPE of this service admits values the snapshot does not declare. The shapes
that carry it are declared in this crate's own source from a small overlay model beside the
snapshot (`local-additions.json`), so the declaration is generated the same way every
modelled shape is — one member list, one set of constraint traits, no hand-written schema,
serializer or validator. The same mechanism is used by
`services/service-com-amazonaws-eks-awswesleyfrontend`
(`smithy-model/local-additions.json` + `src/ops/types/describe_addon_versions_types.rs`) and
by `services/service-com-amazonaws-pcs-awsparallelcomputingservice`.

## `LogTargetType` admits one more target type

- **What AWS does**: the V2 logging operations take a log target type of `DEFAULT`,
  `THING_GROUP`, `CLIENT_ID`, `SOURCE_IP`, `PRINCIPAL_ID` or `DEVICE_DEFENDER`, and a value
  outside that set is refused with the whole set listed in the clause — which is how a
  caller discovers it.
- **What the snapshot declares**: `LogTargetType` with the first five values only, so a
  request naming the sixth would be refused for a value the service accepts, and a refusal
  would advertise a set narrower than the one the service holds values to.
- **Where it is declared**: the three inputs that carry the type —
  `src/ops/types/delete_v2_logging_level_types.rs`,
  `src/ops/types/list_v2_logging_levels_types.rs`,
  `src/ops/types/set_v2_logging_level_types.rs` — and the `LogTarget` structure two of them
  reach (`src/ops/types/shared_types.rs`), each naming `local-additions.json`, which
  restates those shapes against the snapshot's own members, `@required` markings and
  `@httpQuery` bindings so each request is read exactly as the snapshot binds it, with the
  wider value set on the one enum. The value set a caller reads off a refusal is stated in
  the order the front door walks its table of the admissible values
  (`service_common::validation_message::in_hashed_enum_order`), which is derived from the
  values themselves — so the added value needs nothing beyond this declaration.

## What else is in the overlay

Only the shapes those member lists target, and only as much of each as codegen reads:
`LogTargetName` and `NextToken` (strings), `SkyfallMaxResults` with the `@range` the
snapshot spells for it, and `LogLevel` with the five values the snapshot declares.

`tests/all/log_target_type_value_set.rs` pins the wider set on the wire — every target type
served by the operations that take one, and a string outside the set refused with the whole
set listed, beside `tests/all/v2_logging_options/logging_levels.rs`, which drives every value
of the set through the writer, the reader and the remover. If the snapshot is ever refreshed
with the sixth value, the local declarations should give way to it: drop the `model = …`
argument from the four declarations and delete the overlay entries.
