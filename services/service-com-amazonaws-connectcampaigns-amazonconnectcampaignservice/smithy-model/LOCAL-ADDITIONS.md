# Local additions to the Smithy model snapshot

`com-amazonaws-connectcampaigns-amazonconnectcampaignservice.json` in this directory is a
snapshot of the published Amazon Connect Campaigns (v1) model and is kept EXACTLY as
shipped: it is a third-party artefact consumed at build time by
`#[smithy_macros::smithy]`, so an edit there would be invisible to a reader of the crate's
source and lost on the next refresh.

One member of this service is held to a constraint the published snapshot does not state.
It is declared in this crate's own source instead, from a small overlay model beside the
snapshot (`local-additions.json`), so the declaration is generated the same way every
modelled shape is — one member list, one set of constraint traits, no hand-written schema,
serializer or validator.

## `GetCampaignStateBatchRequest$campaignIds` is bounded at five hundred ids

- **What AWS does**: a batch state read ANSWERS a batch of up to five hundred campaign
  ids — every id the account holds no campaign under coming back in `failedRequests` — and
  refuses a larger batch with HTTP 400 `ValidationException`, whose clause names the
  submitted list and five hundred as the maximum.
- **What the snapshot declares**: `@length(min: 1, max: 25)` on that member, which is the
  number the member's documentation prints. Held to it, the request walk in front of the
  operation refuses a batch the recorded service answers, so a legitimate batch read is
  impossible and the refusal that IS correct names a limit the service does not apply.
- **Where it is declared**: `src/ops/types/get_campaign_state_batch_types.rs`, which holds
  this operation's input type. It is generated from `local-additions.json`, which restates
  `GetCampaignStateBatchRequest` against the snapshot's own target shapes — so the entry
  length and pattern of a `CampaignId`, the list's own minimum of one and the member being
  `@required` are captured exactly as the snapshot spells them — with the maximum the
  service enforces.
- **What else is in the overlay**: only the shapes that member targets, and only as much of
  each as codegen reads: the list, and the string its elements are, with its length and
  pattern.

`tests/all/batch_size_ceiling.rs` pins the ceiling on the wire — the sizes up to it that are
answered, one entry per id, and the sizes past it that are refused naming it — beside
`tests/all/list_member_length_bounds.rs`, which holds every list member of this service to
the bounds it publishes. If the snapshot is ever refreshed with the five-hundred maximum,
the local declaration should give way to the modelled shape, and the derive in that types
module should go back to naming the snapshot alone.
