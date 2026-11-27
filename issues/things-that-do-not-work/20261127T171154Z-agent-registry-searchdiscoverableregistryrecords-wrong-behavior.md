Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: eca447fb6703ac098f99a805138e6e6e630c3cd7

# KEYWORD/HYBRID ranking reads only name, displayName and description — the descriptors and custom-metadata text the model names are never matched

_Filed: 2026-11-27T17:11:54Z_
_Operation: SearchDiscoverableRegistryRecords | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `eca447fb6703ac098f99a805138e6e6e630c3cd7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model defines the KEYWORD ranking over four sources of record text: the record's name, its description, its DESCRIPTORS and its CUSTOM METADATA text values. The handler's `searchable_text` reads only three fields (name, displayName, description) and never looks at `record.descriptors` or `record.custom_metadata`.

Because `keyword_rank` returns `None` when a term occurs in none of those three fields, and `SearchMode::rank` drops a record on `keyword_rank(...)?`, a record whose only match is in a descriptor payload (e.g. an MCP tool name) or in a custom-metadata text value is not ranked low — it is EXCLUDED from the result set entirely. This hits both KEYWORD and HYBRID, and HYBRID is the handler's default for a request that names no `searchType` (`DEFAULT_SEARCH_TYPE: SearchMode = SearchMode::Hybrid`), so it is the default path. The model points exactly at this use case: "Use this type for queries built from specific terms, such as tool names or API names" — tool names are descriptor content, not the record name.

The data is already in hand in the same handler: the metadata-filter path reads `record.custom_metadata` (`field_value`, search_discoverable_registry_records.rs:680-686) and the projection carries the full `descriptors` structure (state.rs:256), so the ranking reads a narrower set of text than the crate already holds and than the model documents.

SEMANTIC is affected in the same way: `similarity` is computed over the same three-field array, so the similarity ranking also ignores descriptor and metadata text.

## Evidence

Smithy, `SearchType$KEYWORD` (smithy_query(query="doc", shape="SearchType", member="KEYWORD")):
  "Ranks records by keyword match between the query terms and the record name, description, descriptors, and custom metadata text values. Use this type for queries built from specific terms, such as tool names or API names."
The same enumeration is repeated in `SearchDiscoverableRegistryRecordsRequest$searchType`:
  "KEYWORD – Ranks records by keyword match between the query terms and the record name, description, descriptors, and custom metadata text values."

Handler, services/service-com-amazonaws-agentregistry-agentregistry/src/ops/search_discoverable_registry_records.rs:473-475:
  /// The record fields a `searchQuery` is matched against, most specific first: `name`,
  /// `displayName`, `description` — the text members `RegistryRecordSummary` publishes.
  const SEARCHABLE_FIELDS: usize = 3;
and :494-500
  fn searchable_text(record: &RegistryRecord) -> [Option<String>; SEARCHABLE_FIELDS] {
      [
          Some(record.name.to_lowercase()),
          record.display_name.as_deref().map(str::to_lowercase),
          record.description.as_deref().map(str::to_lowercase),
      ]
  }
No read of `record.descriptors` or `record.custom_metadata` occurs anywhere in the ranking path.

That a non-match is an EXCLUSION, not a demotion — :416-422
  Self::Keyword => (keyword_rank(&searchable, terms)?, 0.0),
  ...
  Self::Hybrid => (keyword_rank(&searchable, terms)?, similarity(&searchable, search_query)),
with :525-527
  if !terms.iter().all(occurs_anywhere) {
      return None;
  }
and the caller dropping the record at :273-276
  let Some(ranked_record) = search_type.rank(record, &terms, &input.search_query) else { continue; };

HYBRID is the default path — :338
  const DEFAULT_SEARCH_TYPE: SearchMode = SearchMode::Hybrid;

The crate already holds both ignored sources: src/state.rs:256 `pub descriptors: Descriptors,` / :259 `pub custom_metadata: Option<Document>,` and the filter path reads the latter at search_discoverable_registry_records.rs:681-685 (`field.strip_prefix(CUSTOM_METADATA_PREFIX)` ... `metadata.get(key)`).

Test coverage: tests/all/discovery_read_path.rs:2146-2160 asserts the contrast case only for a query ("tide forecast harbor") that matches no field of the seeded records at all, so widening the matched text set leaves that assertion intact; the multi-word term-matching rule pinned at :1837-1841 is likewise unaffected (it is about terms vs. whole-phrase, not about which fields are read).

## Steps to Reproduce

1. Through AgentRegistryControl, create a registry and a record whose `name`/`displayName`/`description` contain none of the words of the query, but whose `descriptors` payload (e.g. an mcpServer `data` document naming a tool) or whose `customMetadata` text value contains them; approve the record so it is APPROVED and discoverable.
2. Call SearchDiscoverableRegistryRecords with `searchQuery` set to that tool name, `registryIds=[<registry>]`, `searchType="KEYWORD"`.
3. Observe HTTP 200 with `registryRecords: []` — the record is excluded, because keyword_rank returned None.
4. Repeat with `searchType` omitted (HYBRID, the handler's default) and with `searchType="SEMANTIC"`: the record is likewise never ranked against descriptor or custom-metadata text.
5. Expected per the model: the record is ranked by keyword match against "the record name, description, descriptors, and custom metadata text values" and appears in the page.
