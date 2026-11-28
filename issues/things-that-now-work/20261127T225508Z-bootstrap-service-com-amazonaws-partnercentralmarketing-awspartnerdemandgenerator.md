Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: d16cb6b48ca442f386de2692dccb3e1096ba2b34

# Bootstrap the service-com-amazonaws-partnercentralmarketing-awspartnerdemandgenerator emulator

_Filed: 2026-11-27T22:55:08Z_
_Operation: service-com-amazonaws-partnercentralmarketing-awspartnerdemandgenerator | Class: missing_

> _Note: this issue was identified against the code base at commit `d16cb6b48ca442f386de2692dccb3e1096ba2b34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 23 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501, and `src/state.rs` holds no fields (`// TODO: add service-specific state fields`). The emulator therefore has no working behaviour at all.

Implement a FIRST, SMALL happy-path slice around the service's primary resource, the campaign. Scope = these 6 operations, in this order:

1. **SearchCampaignTemplates** — input `Catalog` (required, AWS|Sandbox), optional filters `SearchText`, `Industry`, `UseCase`, `Language`, `AwsServices`, `CustomerSegments`, `Themes`, `Collection`, plus `MaxResults` (1..100) and `NextToken`. Output `TotalResults` (required), `CampaignTemplates` (list<CampaignTemplateSummary>, required), `NextToken`. Templates are platform-managed ("an AWS-authored kit of asset blueprints a partner creates a campaign from"), so seed a small fixed catalog of templates into state at startup and return them FROM that state — paginated honestly: slice by `MaxResults`, emit `NextToken` only when more remain, resume from it, and set `TotalResults` from the filtered count.
2. **GetCampaignTemplate** — input `Catalog`, `Identifier` (`^tmpl-[0-9a-z]{13}$`). Output `CampaignTemplate` (required) read from the seeded catalog; `ResourceNotFoundException` (404) for an unknown id, and also for a template whose `EffectiveDateBegin` is in the future (the model says: "Until then the template does not exist to a partner: SearchCampaignTemplates omits it and GetCampaignTemplate returns ResourceNotFoundException").
3. **CreateCampaign** — input `Catalog`, `Name`, `CampaignTemplateId` (all required) plus `SenderEmail`, `PrivacyPolicyUrl`, `PartnerAddress`, `StartOn`, `EndOn`, `EstimatedBudget`, `TotalBudget`, `TargetAudience`, `EmailTargetEngagementGroupIds`, `ClientToken`, `Tags`. Must look up `CampaignTemplateId` in the seeded catalog and return `ResourceNotFoundException` when absent; mint `Id` matching `^camp-[A-Za-z0-9]{13}$` and `Arn` matching the modelled ARN pattern (`arn:aws:partnercentral:<region>:<account>:catalog/<Catalog>/campaign/<Id>`); PERSIST the whole campaign; return only `Id` + `Arn` ("Returns the identity of the new campaign, not the campaign"). Honour `ClientToken` (`@idempotencyToken`): a repeat with the same token returns the same `Id`/`Arn` without creating a second campaign.
4. **GetCampaign** — input `Catalog`, `CampaignIdentifier` (bare `camp-...` OR the full ARN, per the pattern `^(arn:...campaign/)?camp-[A-Za-z0-9]{13}$` — accept both forms). Output `Campaign` (required) rebuilt from stored state: `CampaignId`, `Arn`, `Name`, `State`, `CampaignTemplateId`, `Language` (taken from the template's `LanguageCode`), `SenderEmail`, `PrivacyPolicyUrl`, `PartnerAddress`, `StartOn`, `EndOn`, budgets, `TargetAudience`, `CreatedAt`, `UpdatedAt`, and the required `ActivationIssues` list (empty when nothing is missing). `State` must be DERIVED on every read, not stored: `COMPLETED` once the UTC date is after `EndOn`, `ACTIVE` when at least one asset is live, otherwise `DRAFT`. 404 for an unknown/deleted id.
5. **UpdateCampaign** — input `Catalog`, `CampaignIdentifier` (required) and the mutable fields `Name`, `SenderEmail`, `PrivacyPolicyUrl`, `PartnerAddress`, `StartOn`, `EndOn`, budgets, `TargetAudience`, `EmailTargetEngagementGroupIds`, `SocialConnections`, `ClientToken`. Apply only the members present, leave absent members untouched, bump `UpdatedAt`, reject an unknown id with `ResourceNotFoundException`, and return `Id`. `CampaignTemplateId` is immutable ("Set at create, immutable") and is not an input here. A following GetCampaign must show the new values.
6. **ListCampaigns** — input `Catalog`, optional `CampaignIdentifiers`, `Name`, `States`, `Sort`, `MaxResults`, `NextToken`. Output `Items` (list<CampaignSummary>, required) + `NextToken`. Build each summary from stored state (`CampaignId`, `Arn`, `Name`, derived `State`, `Language`, `StartOn`, `EndOn`, `CreatedAt`, `UpdatedAt`), apply the filters, apply `Sort`, and paginate with a real resumable token.
7. **DeleteCampaign** — input `Catalog`, `CampaignIdentifier`. Removes the campaign (and its tags/assets) from state; empty output. Being `@idempotent`, a second delete of the same id must not panic; a GetCampaign afterwards must return `ResourceNotFoundException`.

Requirements for the whole slice: NO canned, echoed or hardcoded responses. Every response field must be computed from persisted state held in the shared engine reached via `crate::state::service_state(&*self.engine)` (never `InMemoryEngine::new()` inside a handler). Identifiers must be minted to match the modelled patterns and then stored, not generated and thrown away. The operations outside this list (asset CRUD, activation/export tasks, social connections, tagging) correctly stay at 501 and are filed separately.

## State Model

In `ComAmazonawsPartnercentralmarketingAwspartnerdemandgeneratorState` (currently empty at `src/state.rs:7-9`):

- `campaigns: HashMap<String /* catalog */, HashMap<String /* campaign id `camp-...` */, Campaign>>` where the stored `Campaign` record holds: `id`, `arn`, `name`, `catalog`, `template_id`, `language`, `sender_email`, `privacy_policy_url`, `partner_address`, `partner_badge`, `email_target`, `email_target_engagement_group_ids: Vec<String>`, `social_connections: HashMap<Platform, Vec<String>>`, `estimated_budget`, `total_budget`, `target_audience`, `start_on: Option<String>`, `end_on: Option<String>`, `created_at`, `updated_at`, `tags: Vec<Tag>`, `assets: Vec<CampaignAssetRecord>` (empty in this slice; the state field exists so `State` derivation and ListCampaignAssets have one home).
- `templates: Vec<CampaignTemplateRecord>` — the seeded, read-only catalog: `id` (`tmpl-...`), `name`, `industry`, `use_case`, `description`, `language_code`, `collection`, `aws_services`, `customer_segments`, `themes`, `setup_duration`, `thumbnail_url`, `effective_date_begin`, `asset_templates`, `reference_materials`, `created_at`, `updated_at`. Seeded once on first access.
- `idempotency: HashMap<String /* ClientToken */, String /* minted campaign id */>` so `CreateCampaign`/`UpdateCampaign`/`DeleteCampaign` retries are idempotent as modelled.
- `next_id: u64` or an RNG seed used to mint pattern-conforming `camp-...` ids.

Done = an integration test that runs the workflow below end to end and asserts BODY contents (not just 2xx).

## Evidence

All 23 handlers are scaffolds. e.g. `services/service-com-amazonaws-partnercentralmarketing-awspartnerdemandgenerator/src/ops/create_campaign.rs:4`: `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and` — the same header appears in get_campaign.rs:4, list_campaigns.rs:4, update_campaign.rs:4, delete_campaign.rs:4, search_campaign_templates.rs:4, get_campaign_template.rs:4 and every other file in `src/ops/`.

No state exists yet: `src/state.rs:7-9`:
```
pub struct ComAmazonawsPartnercentralmarketingAwspartnerdemandgeneratorState {
    // TODO: add service-specific state fields
}
```

Contract for the chosen slice, from `OPERATIONS.md`:
- line 82-84 CreateCampaign: "**Input:** `Catalog` (... required), `Name` (string, len 1..100, required), `CampaignTemplateId` (string, len 18..18, pattern ^tmpl-[0-9a-z]{13}$, required) ..." / "**Output:** `Id` (string, len 18..18, pattern ^camp-[A-Za-z0-9]{13}$, required), `Arn` (...required)" / errors `ConflictException`, `ResourceNotFoundException`, `ServiceQuotaExceededException`, `ValidationException`.
- line 89-91 GetCampaign: "**Input:** `Catalog` ..., `CampaignIdentifier` (string, len 18..200, pattern ^(arn:...campaign/)?camp-[A-Za-z0-9]{13}$, required)" / "**Output:** `Campaign` (Campaign, required)" / "**Errors:** `ResourceNotFoundException (HTTP 404)`".
- line 96-98 UpdateCampaign (@idempotent) and line 103-105 DeleteCampaign (@idempotent).
- line 109-112 ListCampaigns: "@paginated(inputToken=NextToken, outputToken=NextToken, items=Items), @readonly" with `MaxResults` (range 1..100).
- line 61-64 SearchCampaignTemplates: "@paginated(...items=CampaignTemplates)" / output "`TotalResults` (integer, required), `CampaignTemplates` (...required), `NextToken`".
- line 41-43 GetCampaignTemplate: output "`CampaignTemplate` (CampaignTemplate, required)", error `ResourceNotFoundException (HTTP 404)`.

Smithy docs grounding the derived/seeded behaviour:
- `CreateCampaign` doc: "Returns the identity of the new campaign, not the campaign. ... the caller's next step is a `GetCampaign` anyway".
- `Campaign$State` / `CampaignSummary$State` doc: "The campaign's state, derived on every read from `EndOn` and its assets."; `CampaignState` doc: "`COMPLETED` once the UTC date is after `EndOn` ... otherwise `ACTIVE` wh[en at least one asset is live]" with members DRAFT/ACTIVE/COMPLETED.
- `Campaign$CampaignTemplateId` doc: "The campaign template this campaign draws its assets from. Set at create, immutable."
- `Campaign$Language` doc: "The language the campaign's assets are written and typeset in: its template's `LanguageCode`, set at create."
- `CampaignTemplate` doc: "A platform-managed campaign template — an AWS-authored kit of asset blueprints a partner creates a campaign from."; `CampaignTemplate$EffectiveDateBegin` doc: "Until then the template does not exist to a partner: SearchCampaignTemplates omits it and GetCampaignTemplate returns ResourceNotFoundException."
- `CampaignSummary$Arn` doc: "Present on the summary so a caller can tag a campaign straight from a list result without a GetCampaign per row".

Test coverage today: only `tests/all/state_roundtrip.rs` (a compiled example of the state API, per README.md:20-22) — no behavioural test exists.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK client at the `partnercentral-marketing` endpoint with `Catalog=Sandbox`.
2. Call SearchCampaignTemplates(Catalog=Sandbox, MaxResults=1) — expect a non-empty `CampaignTemplates` with a `TotalResults` >= 1 and, when more templates exist, a `NextToken`; pass that token back and expect the NEXT template, not the first one again. Today: HTTP 501 NotImplemented.
3. Call GetCampaignTemplate(Catalog=Sandbox, Identifier=<id from the search>) — expect the full `CampaignTemplate` with that `Id`, `Name` and `Collection`. Call it with `tmpl-0000000000000` and expect ResourceNotFoundException (404).
4. Call CreateCampaign(Catalog=Sandbox, Name="demo", CampaignTemplateId=<that id>, SenderEmail="a@b.com", StartOn="2025-01-01", EndOn="2099-12-31", ClientToken="tok-1") — expect `Id` matching ^camp-[A-Za-z0-9]{13}$ and an `Arn` containing `catalog/Sandbox/campaign/<Id>`.
5. Repeat the identical CreateCampaign with ClientToken="tok-1" — expect the SAME `Id`/`Arn` and no second campaign in ListCampaigns.
6. Call GetCampaign(Catalog=Sandbox, CampaignIdentifier=<Id>) — expect `Campaign.Name == "demo"`, `CampaignTemplateId` == the template used, `Language` == the template's `LanguageCode`, `State == DRAFT` (EndOn in the future, no live asset), `CreatedAt` set, and `ActivationIssues` present (possibly empty).
7. Call GetCampaign again passing the full `Arn` as `CampaignIdentifier` — expect the identical campaign.
8. Call UpdateCampaign(Catalog=Sandbox, CampaignIdentifier=<Id>, Name="demo-renamed") — expect `Id` back; then GetCampaign must show `Name == "demo-renamed"`, an unchanged `SenderEmail`, and an `UpdatedAt` later than before.
9. Call ListCampaigns(Catalog=Sandbox) — expect one `CampaignSummary` with that `CampaignId`, `Arn`, `Name == "demo-renamed"` and derived `State`. Call ListCampaigns(Name="nope") and expect an empty `Items`.
10. Call DeleteCampaign(Catalog=Sandbox, CampaignIdentifier=<Id>) — expect success; then GetCampaign(<Id>) must return ResourceNotFoundException (404), and ListCampaigns must return empty `Items`. A second DeleteCampaign must not panic.
