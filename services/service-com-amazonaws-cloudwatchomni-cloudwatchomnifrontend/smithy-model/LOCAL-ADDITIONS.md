# Local additions to the Smithy model snapshot

`com-amazonaws-cloudwatchomni-cloudwatchomnifrontend.json` in this directory is a snapshot
of the published model and is kept EXACTLY as shipped: it is a third-party artefact consumed
at build time by `#[smithy_macros::smithy]`, so an edit there would be invisible to a reader
of the crate's source and lost on the next refresh of the snapshot.

One member of this service is held to a constraint WIDER than the published snapshot states.
The shapes that carry it are declared in this crate's own source instead, from a small
overlay model beside the snapshot (`local-additions.json`), so the declaration is generated
the same way every modelled shape is — one member list, one set of constraint traits, no
hand-written schema, serializer or validator. The same mechanism is used by
`services/service-com-amazonaws-eks-awswesleyfrontend`
(`smithy-model/local-additions.json` + `src/ops/types/describe_addon_versions_types.rs`) and
by `services/service-com-amazonaws-pcs-awsparallelcomputingservice`.

## `ViewName` admits the period among its trailing characters

- **What AWS does**: a view name is the `view.` prefix, one lowercase alphanumeric character
  and then up to 250 characters drawn from the lowercase alphanumerics, the hyphen, the
  underscore AND the period — which is the expression a refusal of a name outside the class
  advertises, and so the rule a client that compiles that expression derives. Dotted names
  (`view.sales.weekly`) are served.
- **What the snapshot declares**: `@pattern("^view\.[a-z0-9][a-z0-9_-]{0,250}$")` on
  `ViewName`, without the period. Held to it, the front door refuses names the service serves
  and advertises an expression narrower than the one it enforces.
- **Where it is declared**: the four inputs that carry a view name —
  `src/ops/types/create_view_types.rs`, `src/ops/types/get_view_types.rs`,
  `src/ops/types/update_view_types.rs`, `src/ops/types/delete_view_types.rs` — each naming
  `local-additions.json`, which restates those structures against the snapshot's own members,
  targets and `@required` markings, with the period inside `ViewName`'s trailing class. The
  length bounds, the idempotency token and every other constraint are the snapshot's own, so
  only the one character class differs. The refusal's wording is untouched: the expression a
  clause quotes is the `@pattern` of the member that was refused
  (`service_common::validation_message`), so it follows the declaration.

## What else is in the overlay

Only the shapes those member lists target, and only as much of each as codegen reads:
`ViewDefinition`, `ViewDescription`, `TagMap` with its key and value shapes, each with the
length traits the snapshot spells for them. The idempotency token targets the Smithy
prelude's `String`, which no model file declares, and keeps its own member traits.

`tests/all/view_name_rule.rs` pins both halves on the wire — which names every operation
carrying a view name admits, and that the expression a refusal advertises decides exactly
the names the front door decides. If the snapshot is ever refreshed with the period in the
class, the local declarations should give way to it: drop the `model = …` argument from the
four declarations and delete the overlay entries.
