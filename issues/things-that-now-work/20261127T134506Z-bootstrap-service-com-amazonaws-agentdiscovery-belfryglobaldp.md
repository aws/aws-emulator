Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: bb517dad8aa2a6dcfbb56aa45c4ce46fe44a83f5

# Bootstrap the service-com-amazonaws-agentdiscovery-belfryglobaldp emulator

_Filed: 2026-11-27T13:45:06Z_
_Operation: service-com-amazonaws-agentdiscovery-belfryglobaldp | Class: stub_

> _Note: this issue was identified against the code base at commit `bb517dad8aa2a6dcfbb56aa45c4ce46fe44a83f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has exactly ONE modeled operation, so the bootstrap happy path IS that operation implemented statefully end-to-end:

1. **SearchPublicRecords** (`POST /records/search`, restJson1, `@readonly`, `@paginated(inputToken=nextToken, outputToken=nextToken, items=records, pageSize=maxResults)`)
   - Input: `searchQuery` (string, wire `jsonName: "query"`, len 0..1024), `filters` (document, `SearchFilterExpression`), `maxResults` (integer, range 1..100, doc says "Default 20, max 100"), `nextToken` (string, len 1..2048).
   - Output: `records` (list<PublicRecord>, **required**), `nextToken` (string, len 1..2048, "Absent when there are no more results").
   - Errors available: `ValidationException` (400), `ThrottlingException` (429), `InternalServerException` (500) — each carries `message` and `errorCode` (`ArdErrorCode`: INVALID_ARGUMENT | UNAUTHENTICATED | NOT_FOUND | RATE_LIMIT_EXCEEDED | INTERNAL_ERROR).

The first slice must be a real, stateful search over a persisted record index held in `ComAmazonawsAgentdiscoveryBelfryglobaldpState`, not a canned list and not an echo of the input. Concretely, the implementation must:

- Read the index from `crate::state::service_state(&*self.engine)` under the guard; when `service_state` answers `None`, return `InternalServerException { errorCode: INTERNAL_ERROR }` (never a fresh engine, never a hollow 200).
- Filter the index by `searchQuery` (free-text match over `displayName`/`description`/`domain`) and by `filters`, supporting at least the flat forms the model documents: `$eq`, `$ne`, `$in` on a field, plus `$prefix` on `identifier`, and `$and`/`$or` over single-condition objects (never nested). Fields: `identifier`, `domain`, `recordType`, `descriptorType`, `provenance`, `recordVersion`.
- Reject `{}` and other malformed filter documents with `ValidationException { errorCode: INVALID_ARGUMENT }` — the model notes a `document` cannot carry `@length`, so the handler owns these checks (at most 10 conditions, `$in` 1..25 values, values 1..512 chars).
- Apply `maxResults` (defaulting to 20 when absent) to produce a page, and return an opaque `nextToken` ONLY when more matches remain; feeding that token back must resume exactly after the previous page over the same filtered, stably-ordered result set, and the final page must omit `nextToken`.
- `records` is `@required`: an empty result set must serialize an empty list, never a missing member.
- Because the operation is `@readonly`, the handler must not mutate the index — only read it (seeding happens outside the handler).

## State Model

Add to `ComAmazonawsAgentdiscoveryBelfryglobaldpState` (src/state.rs, which today is an empty struct with `// TODO: add service-specific state fields`):

- `records: Vec<PublicRecordEntry>` — the public record index, in stable insertion order so paging is deterministic. Also expose a seeding entry point (e.g. `insert_record`) used by tests/fixtures, since no modeled write operation exists.
- `PublicRecordEntry` mirrors the modeled `PublicRecord`: `identifier: Identifier` (union of `domain: DomainName` | `uri: identifierUri`), `display_name: String` (required, 1..255), `description: Option<String>` (0..4096), `record_type: RecordType` (AGENT | TOOL | SKILL | CUSTOM | GATEWAY | WEB_BOT, required), `domain: Option<DomainName>`, `descriptors: BTreeMap<String, DescriptorEntry>` (≤32 slots; `DescriptorEntry{ data, version, source: Option<DescriptorSource{fromUrl: DescriptorSourceUrl{url}}>, additional_data }`), `trust_manifests: Vec<TrustManifest>` (≤10; `TrustManifest{ contributor: trustUri (required), provenance: Vec<ProvenanceLink{ relation: publishedFrom|discoveredFrom|federatedFrom, source_id: trustUri }> }`), `record_version: Option<String>`, `created_at: Option<Timestamp>`, `updated_at: Option<Timestamp>`.
- `relevance`/ordering: a deterministic score or stable index per entry so "most relevant first" is reproducible across pages.
- Pagination cursor: an opaque `nextToken` encoding the offset into the filtered result set plus a digest of (`searchQuery`, `filters`, `maxResults`) so a token replayed against different criteria is refused with `ValidationException`.

Done = an integration test that seeds several `PublicRecordEntry` values into the shared engine state, then (a) searches with no filters and `maxResults` smaller than the seeded count and walks every page via `nextToken` until it is absent, asserting each returned record's fields equal what was seeded and no record is duplicated or skipped; (b) searches with `{"recordType": {"$eq": "AGENT"}}` and gets only AGENT records; (c) searches with a query that matches nothing and gets an empty `records` list with no `nextToken`; (d) sends `filters: {}` and gets HTTP 400 `ValidationException` with `errorCode: INVALID_ARGUMENT`.

## Evidence

Handler is an unmodified scaffold: `services/.../src/ops/search_public_records.rs:4` — "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and asserts NOTHING about `SearchPublicRecords`'s real behaviour." and :180 — `Err(crate::scaffolded::Scaffolded::not_implemented("SearchPublicRecords"))`. It is the only handler file in `src/ops/` besides `mod.rs`.

State is empty: `services/.../src/state.rs:7-9` — `pub struct ComAmazonawsAgentdiscoveryBelfryglobaldpState { // TODO: add service-specific state fields }`.

Contract, from `OPERATIONS.md:25-31`: "## SearchPublicRecords — **Route:** `POST /records/search` — **Traits:** @paginated(inputToken=nextToken, outputToken=nextToken, items=records), @readonly — **Input:** `searchQuery` (string, len 0..1024), `filters` (document), `maxResults` (integer, range 1..100), `nextToken` (string, len 1..2048) — **Output:** `records` (list<PublicRecord>, required), `nextToken` (string, len 1..2048) — **Errors:** `InternalServerException (HTTP 500)`, `ThrottlingException (HTTP 429)`, `ValidationException (HTTP 400)`". Protocol from `OPERATIONS.md:10-12`: "**Protocol:** `restJson1` ... **sigv4 signing name:** `agent-discovery`".

Modeled docs (search_public_records.rs:52-78): "Search public agent records across the authoritative + crawler indexes."; `filters` — "`$eq`, `$ne` and `$in` on a field, with `$and` or `$or` over single-condition objects, never nested. Fields are `identifier`, `domain`, `recordType`, `descriptorType`, `provenance` and `recordVersion`, and `identifier` also takes `$prefix`. ... at most 10 conditions, `$in` takes 1-25 values, values are strings of 1-512 characters, and `{}` is invalid. A `document` can't carry `@length`, so the service …"; `maxResults` — "Max records per page. Default 20, max 100."; output `nextToken` — "The token to pass in a later request to get the next page of results. Absent when there are no more results."; `records` — "**required** — The records that match the search, most relevant first."

Shape detail from smithy_query closure of `SearchPublicRecords`: `searchQuery` member carries `"jsonName": "query"`; `PublicRecord` requires `identifier` (union `Identifier{domain|uri}`), `displayName`, `recordType`; `ValidationException`/`ThrottlingException`/`InternalServerException` each have `message` + `errorCode` of enum `ArdErrorCode` with values [INVALID_ARGUMENT, UNAUTHENTICATED, NOT_FOUND, RATE_LIMIT_EXCEEDED, INTERNAL_ERROR].

## Steps to Reproduce

1. Start the emulator with this service mounted.
2. Seed the record index in the shared engine state with several PublicRecord entries of mixed recordType (AGENT, TOOL, GATEWAY) and domains.
3. POST /records/search with body {"query": "", "maxResults": 2} — today this answers HTTP 501 NotImplemented; it must answer 200 with 2 seeded records and a nextToken.
4. Re-POST /records/search passing the returned nextToken until the response omits nextToken; assert the concatenated pages equal the seeded set exactly, with no duplicates or omissions.
5. POST /records/search with {"filters": {"recordType": {"$eq": "AGENT"}}} and assert only AGENT records come back, each field matching what was seeded.
6. POST /records/search with {"query": "no-such-agent-xyz"} and assert 200 with records: [] and no nextToken.
7. POST /records/search with {"filters": {}} and assert HTTP 400 ValidationException with errorCode INVALID_ARGUMENT (the model says `{}` is invalid).
