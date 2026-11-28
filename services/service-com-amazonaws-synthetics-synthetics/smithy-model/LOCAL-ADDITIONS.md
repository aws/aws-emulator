# Local additions to the Smithy model snapshot

`com-amazonaws-synthetics-synthetics.json` in this directory is a snapshot of the
published Synthetics model and is kept EXACTLY as shipped: it is a third-party
artefact consumed at build time by `#[smithy_macros::smithy]`, so an edit there
would be invisible to a reader of the crate's source and lost on the next
refresh — as one duly was, which is why this overlay exists.

AWS answers one of this service's reads with one member MORE than that snapshot
declares. The shape that carries it is declared in this crate's own source
instead, from a small overlay model beside the snapshot
(`local-additions.json`), so the declaration is generated the same way every
modelled shape is — one member list, one set of constraint traits, no
hand-written schema or serializer. The same mechanism is used by
`services/service-com-amazonaws-cloudformation-cloudformation` and
`services/service-com-amazonaws-arczonalshift-percdataplane`
(`smithy-model/local-additions.json` + a types file at a generated path).

## `RuntimeVersion.Language`

- **What AWS does**: every entry of the canary runtime catalog
  `DescribeRuntimeVersions` reports names the language a canary script on that
  runtime version is written in, in the member `Language` — `nodejs` for the
  Node.js families (Playwright, Puppeteer and the retired `syn-nodejs-*` ones)
  and `python` for the Python with Selenium family. A client that splits the
  catalog by language, or that picks the newest runtime of the language its
  script is written in, reads that member.
- **What the snapshot declares**: `RuntimeVersion` with `VersionName`,
  `Description`, `ReleaseDate` and `DeprecationDate`, none of them `Language`,
  and no shape of that name at all.
- **Where it is declared**: `src/ops/types/describe_runtime_versions_types.rs`,
  whose `RuntimeVersion` is generated from `local-additions.json`. That overlay
  restates the four modelled members against the snapshot's own targets — so
  their length constraints are captured exactly as the snapshot spells them —
  and adds `Language`: an open `string`, because nothing publishes a value set
  for it. The VALUE is not declared there either; it is read off the runtime
  FAMILY the version name states (`crate::state::runtime_language`), so the
  language an entry reports cannot disagree with the name beside it.
- **What else is in the overlay**: only the shapes those members target, and
  only as much of each as codegen reads — the string with its length, the
  timestamp with its type.

The input and output shapes of the operation are unchanged, so they are still
generated from the snapshot: the file names the overlay on the ONE attribute that
needs it (`model = "smithy-model/local-additions.json"`). That file sits at a path
the scaffold generator also writes and carries no `@generated` marker: the marker
is what tells the next model refresh a file is rendered from the snapshot and may
be overwritten, so a hand-written declaration at such a path must not carry one.

`tests/all/runtime_catalog.rs` pins both halves: the member on the wire, over
every family and both languages the catalog publishes; and the overlay against
the snapshot, shape by shape (type, members and constraint traits, documentation
aside). If the snapshot is ever refreshed with `Language` declared, with a member
the overlay does not restate, or with a changed target or constraint, that suite
fails — and the local declaration should then give way to the modelled shape.
