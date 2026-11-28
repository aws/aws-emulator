# Local additions to the Smithy model snapshot

`com-amazonaws-imagebuilder-imagebuilder.json` in this directory is a snapshot of the
published EC2 Image Builder model and is kept EXACTLY as shipped: it is a third-party
artefact consumed at build time by `#[smithy_macros::smithy]`, so an edit there would be
invisible to a reader of the crate's source and lost on the next refresh of the snapshot.

One member of every listing of this service is held to a constraint NARROWER than that
snapshot states. The shapes that carry it are declared in this crate's own source instead,
from a small overlay model beside the snapshot (`local-additions.json`), so the declaration
is generated the same way every modelled shape is — one member list, one set of constraint
traits, no hand-written schema, serializer or validator. The same mechanism is used by
`services/service-com-amazonaws-eks-awswesleyfrontend`
(`smithy-model/local-additions.json` + `src/ops/types/describe_addon_versions_types.rs`) and
by `services/service-com-amazonaws-pcs-awsparallelcomputingservice`.

## `RestrictedInteger` — the page size every listing takes — stops at 25

- **What AWS does**: a listing refuses a `maxResults` above 25 with
  `InvalidParameterValueException` (HTTP 400) in this API's own parameter sentence ("The
  value supplied for parameter 'maxResults' is not valid. maxResults must be smaller than or
  equal to 25"), and serves every page size from 1 to 25. The bound belongs to the page size
  rather than to one operation, so every listing that takes one applies it.
- **What the snapshot declares**: `@range(min: 1, max: 100)` on `RestrictedInteger`, the
  shape every listing's `maxResults` targets, with the member's documentation printing "The
  largest page size that you can request is 100 items." Held to it, the request walk serves a
  page size the recorded service refuses.
- **Where it is declared**: the twenty listing inputs that carry a page size — the
  `src/ops/types/list_*_types.rs` modules — each naming `local-additions.json`, which
  restates those structures against the snapshot's own members, targets and `@required`
  markings, with `RestrictedInteger` carrying the maximum the service enforces. The
  refusal's wording is untouched: the clause states the bound read off the violation
  (`src/service/parameter_refusal.rs`), so it follows the declaration.
- **What the handlers see**: `crate::state::MAX_LIST_PAGE_SIZE` is that same maximum — the
  bound a handler clamps its page to, now that no larger page size reaches one.

## What else is in the overlay

Only the shapes those member lists target, and only as much of each as codegen reads:
`PaginationToken`, `NonEmptyString`, `Boolean` (with the `@default` the snapshot gives it),
`NullableBoolean`, `Ownership` with its five values IN THE ORDER the snapshot declares them
(which is the order a refusal lists them in), the filter list, filter and filter-value
shapes with their length and pattern traits, and the ARN and id shapes the eight listings
addressed by a resource carry — each with the pattern the snapshot spells for it, because a
refusal of such a value quotes that expression.

`tests/all/listing_page_size.rs` pins both halves on the wire over every listing: the
largest page size each serves, and the refusal of the first one past it. If the snapshot is
ever refreshed with the enforced maximum, the local declarations should give way to it: drop
the `model = …` argument from the twenty declarations and delete the overlay entries.
