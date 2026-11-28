# Local additions to the Smithy model snapshot

`com-amazonaws-eks-awswesleyfrontend.json` in this directory is a snapshot of the published
Amazon EKS model and is kept EXACTLY as shipped: it is a third-party artefact consumed at
build time by `#[smithy_macros::smithy]`, so an edit there would be invisible to a reader of
the crate's source and lost on the next refresh.

One member of this service is held to a constraint the published snapshot does not state.
It is declared in this crate's own source instead, from a small overlay model beside the
snapshot (`local-additions.json`), so the declaration is generated the same way every
modelled shape is — one member list, one set of constraint traits, no hand-written schema,
serializer or validator.

## `DescribeAddonVersionsRequest$maxResults` has no published maximum

- **What AWS does**: the add-on version catalog is answered WHOLE — every add-on that
  matches the request's filters, and no `nextToken` — whatever page size the request asks
  for, including a page size above the hundred the member's documentation prints. The
  listing never pages, so it publishes no maximum page size; a page size below one still
  names no page at all and is refused, in this API's own page-size sentence.
- **What the snapshot declares**: `@range(min: 1, max: 100)` on
  `DescribeAddonVersionsRequestMaxResults`, the shape only this operation targets. Held to
  it, the request walk in front of the operation refuses a request the recorded service
  answers.
- **Where it is declared**: `src/ops/types/describe_addon_versions_types.rs`, which holds
  this operation's input type. It is generated from `local-additions.json`, which restates
  `DescribeAddonVersionsRequest` against the snapshot's own members and `@httpQuery`
  bindings — so the request is read exactly as the snapshot binds it — with the page size's
  minimum alone.
- **What else is in the overlay**: only the shapes those members target, and only as much of
  each as codegen reads: the page-size integer with its remaining bound, and the string list
  the three filter members are.
- **What is NOT in it**: the page-size shapes of the listings that DO page (`ListClusters`,
  `ListAddons`, `ListNodegroups`, `DescribeClusterVersions`). Each keeps the snapshot's own
  `@range(min: 1, max: 100)` and still refuses a page size past it.

`tests/all/listing_page_size_contract.rs` pins both halves on the wire — which listings
refuse a page size past the published maximum and which answer it whole — beside
`tests/all/addon_versions.rs`, which holds the catalog to the same answer at every page size
it admits. If the snapshot is ever refreshed with the unbounded page size, the local
declaration should give way to the modelled shape, and the derive in that types module
should go back to naming the snapshot alone.
