# Local additions to the Smithy model snapshot

`com-amazonaws-socialmessaging-socialmessaging.json` in this directory is a
snapshot of the published AWS End User Messaging Social model and is kept EXACTLY
as shipped: it is a third-party artefact consumed at build time by
`#[smithy_macros::smithy]`, so an edit there would be invisible to a reader of
the crate's source and lost on the next refresh.

AWS answers this service's WhatsApp Business Account listing with one member that
snapshot does not declare. It is declared in this crate's own source instead,
from a small overlay model beside the snapshot (`local-additions.json`), so the
declaration is generated the same way every modelled shape is — one member list,
one set of targets, no hand-written schema or serializer. The same mechanism is
used by `services/service-com-amazonaws-gameliftstreams-gameliftstreams`
(`smithy-model/local-additions.json` +
`src/ops/types/get_stream_group_types/overlay.rs`) and by
`services/service-com-amazonaws-codestarconnections-codestar-connections-20191201`.

## `setupEnabled` on the WhatsApp Business Account listing

- **What AWS does**: `ListLinkedWhatsAppBusinessAccounts`
  (`GET /v1/whatsapp/waba/list`) answers HTTP 200 with a top-level boolean
  `setupEnabled` beside `linkedAccounts` — whether WhatsApp setup is enabled for
  the calling Amazon Web Services account. It is a property of the ACCOUNT, not
  of the page: the recorded listing carries it on an account with no linked
  WhatsApp Business Account at all. A client reads it to decide whether to drive
  the sign-up flow, so an absent member is a missing key rather than a boolean.
- **What the snapshot declares**:
  `ListLinkedWhatsAppBusinessAccountsOutput` with `linkedAccounts` (target
  `LinkedWhatsAppBusinessAccountSummaryList`) and `nextToken` (target
  `NextToken`), and nothing of that name.
- **Where it is declared**:
  `src/ops/types/list_linked_whats_app_business_accounts_types/overlay.rs`,
  generated from `local-additions.json`, which restates the output shape against
  the snapshot's own member targets and adds that one. What the member CARRIES is
  the account-level fact the store holds
  (`crate::state::ComAmazonawsSocialmessagingSocialmessagingState::setup_enabled`),
  read rather than derived from the page, so no value is invented for it.

## What else is in the overlay

Only the shapes those members target, and only as much of each as codegen reads:
the list its member target, the scalar its type.
`LinkedWhatsAppBusinessAccountSummary` is reached ONLY as a list member, where
codegen takes the member list from the member's own Rust type — generated from
the snapshot, in
`src/ops/types/list_linked_whats_app_business_accounts_types/snapshot.rs` — so
the overlay declares it as the structure it is and restates no member of it. The
constraint traits of the scalar target (the token's length) live on the
snapshot's declaration, which is what every request is validated against on the
way in.

The two models are named from two submodules of
`src/ops/types/list_linked_whats_app_business_accounts_types.rs` rather than from
one file because `smithy_macros::smithy_model!` defines the local `smithy!` macro
that `#[smithy]` expands into, and that definition covers the whole file it
appears in — nested modules included.

`tests/all/setup_enabled_listing.rs` pins the member on the wire, over a table of
listings that differ in what the page holds and in how the page was asked for,
and pins the overlay against the snapshot shape by shape: a refreshed snapshot
that declares the member itself, drops a member the overlay restates or changes a
target or type fails there rather than silently narrowing what the service
reports. If the snapshot is ever refreshed with that member, the local
declaration should give way to the modelled one.
