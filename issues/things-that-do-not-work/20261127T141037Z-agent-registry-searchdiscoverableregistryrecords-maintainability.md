Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 2b0c9769718f858c1f805a325f14628bb527d009

# The modelled @length bounds are restated as unreachable handler branches in all three discovery reads, so the copies can drift from the model

_Filed: 2026-11-27T14:10:37Z_
_Operation: SearchDiscoverableRegistryRecords | Class: maintainability_

> _Note: this issue was identified against the code base at commit `2b0c9769718f858c1f805a325f14628bb527d009`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Each of the three discovery-read handlers re-implements a collection bound that the generated input schema already carries and that the boundary validator already enforces, and each handler comment says so. Because the framework refuses the violating request before `handle` runs, none of these branches can ever execute: the constant plus branch is a second, dead copy of a rule the model states once.

The four copies are:

* `SearchDiscoverableRegistryRecords` — `registry_ids.len() != REGISTRY_IDS_PER_REQUEST` against `RegistryIdList` `@length{min:1,max:1}` (`REGISTRY_IDS_PER_REQUEST = 1`, search_discoverable_registry_records.rs:241);
* `BatchGetDiscoverableRegistryRecord` — `entries.len() != ENTRIES_PER_REQUEST` against `entries` `@length{min:1,max:1}` and the `MIN_RECORD_IDS_PER_ENTRY..=MAX_RECORD_IDS_PER_ENTRY` range against `RegistryRecordsEntry$recordIds` `@length{min:1,max:100}`;
* `ListDiscoverableRegistryRecords` — `filters.len() > MAX_FILTERS` against `filters` `@length{max:10}`.

The cost is drift: each constant is documented as a restatement of a model number, nothing links it back to the schema, and a model change would leave the handler constant silently stale (the already-fixed maintainability issue 20261126T153222Z on this same crate is exactly that failure mode for the `maxResults` numbers). Deleting the four branches and the constants that exist only to feed them changes no response and no test — the two refusal tests below already assert the FRAMEWORK's sentence, character for character, for every input that would reach them, which is the proof the branches are unreachable. (The `PAGE_SIZE_BOUNDS` constants that feed `crate::state::page_size` are a different rule — the handler-applied default — and must stay.)

## Evidence

Copy 1 — services/service-com-amazonaws-agentregistry-agentregistry/src/ops/search_discoverable_registry_records.rs:160-173, with the comment admitting the framework refuses first:
```
        // The bounds the model puts on this request, enforced here so the work this
        // handler does is explicitly limited by the request's own modelled limits
        // (the boundary validator refuses a violation before `handle` runs; these
        // checks keep the bound local and readable rather than implicit). With
        // `registryIds` pinned to one, the candidate set is one registry's records.
        if input.registry_ids.len() != REGISTRY_IDS_PER_REQUEST {
            return Err(field_validation(
                "registryIds",
                &format!(
                    "exactly {REGISTRY_IDS_PER_REQUEST} registry identifier must be supplied, got {}.",
                    input.registry_ids.len()
                ),
            ));
        }
```

Copies 2 and 3 — services/service-com-amazonaws-agentregistry-agentregistry/src/ops/batch_get_discoverable_registry_record.rs:151-174, the same comment and the same shape:
```
        // The bounds the model puts on this request, enforced here so the work this
        // handler does is explicitly limited by the request's own modelled limits
        // (the boundary validator refuses a violation before `handle` runs; these
        // checks keep the bound local and readable rather than implicit).
        if input.entries.len() != ENTRIES_PER_REQUEST {
            ...
        for entry in &input.entries {
            let requested = entry.record_ids.len();
            if !(MIN_RECORD_IDS_PER_ENTRY..=MAX_RECORD_IDS_PER_ENTRY).contains(&requested) {
```

Copy 4 — services/service-com-amazonaws-agentregistry-agentregistry/src/ops/list_discoverable_registry_records.rs:157-166:
```
        let filters = input.filters.unwrap_or_default();
        if filters.len() > MAX_FILTERS {
            return Err(field_validation(
                "filters",
                &format!(
                    "at most {MAX_FILTERS} filters may be applied, got {}.",
                    filters.len()
                ),
            ));
        }
```

The other side of the duplication is the model, which the generated input schemas of this crate carry: `smithy_query(query="shape", shape="RegistryIdList")` → `"RegistryIdList": {"type": "list", "member": {...}}` bound at `SearchDiscoverableRegistryRecordsRequest$registryIds` as `"length": {"min": 1, "max": 1}`; `BatchGetDiscoverableRegistryRecordRequest$entries` as `"length": {"min": 1, "max": 1}`; `RegistryRecordsEntry$recordIds` as `"length": {"min": 1, "max": 100}`; `ListDiscoverableRegistryRecordsRequest$filters` as `"length": {"max": 10}`.

Proof the handler branches are unreachable — services/service-com-amazonaws-agentregistry-agentregistry/tests/all/registry_read_refusals.rs:246-292 asserts the framework's clause verbatim for exactly the inputs the handler checks claim to catch:
```
        (
            "entries over its maximum of one",
            "/discoverable-records-batch",
            json!({ "entries": [ ... , ... ] }),
            "Value at 'entries' failed to satisfy constraint: Member must have length less \
             than or equal to 1"
                .to_string(),
        ),
        (
            "registryIds over its maximum of one",
            "/discoverable-records-search",
            json!({ "searchQuery": "", "maxResults": 10,
                    "registryIds": [REGISTRY, "bulkreg00001"] }),
            "Value at 'registryIds' failed to satisfy constraint: Member must have length \
             less than or equal to 1"
                .to_string(),
        ),
        (
            "filters over its maximum of ten",
            &list_path(REGISTRY),
            json!({ "filters": eleven_filters }),
            "Value at 'filters' failed to satisfy constraint: Member must have length less \
             than or equal to 10"
                .to_string(),
        ),
```
and the assertion at :306-311 is an equality, not a `contains`: `let expected = one_error(&clause); let actual = stated(&response); assert_eq!(actual, expected, ...)` where `one_error` (:87-89) is `format!("1 validation error detected: {clause}")`. The nested bound is pinned the same way by `a_nested_collection_outside_its_length_quotes_nothing_either` (:318-349) for `recordIds` over 100 and under 1. No test anywhere in the crate asserts any of the handler-authored sentences (`grep "must be supplied"` over `tests/` returns nothing).

Precedent for the drift this invites, in this same crate: the fixed issue 20261126T153222Z-agent-registry-listdiscoverableregistryrecords-maintainability — "The maxResults default-and-range rule is implemented twice, and the two copies have already drifted".

## Steps to Reproduce

1. Read services/service-com-amazonaws-agentregistry-agentregistry/src/ops/search_discoverable_registry_records.rs:160-173: the handler refuses `registryIds` whose length is not 1 with its own sentence, after a comment stating that "the boundary validator refuses a violation before `handle` runs".
2. POST /discoverable-records-search with `{"searchQuery":"","maxResults":10,"registryIds":["abc123def456","bulkreg00001"]}` and read the body: the message is "1 validation error detected: Value at 'registryIds' failed to satisfy constraint: Member must have length less than or equal to 1" — the framework's clause, asserted by equality in tests/all/registry_read_refusals.rs:266-311 — never the handler's "registryIds: exactly 1 registry identifier must be supplied, got 2.".
3. Repeat for the batch read (`entries` of length 0 and 2, `recordIds` of length 0 and 101) and for the list read (`filters` of length 11): tests/all/registry_read_refusals.rs:246-265, :283-292 and :318-349 assert the framework's clause for each, so none of batch_get_discoverable_registry_record.rs:155-174 or list_discoverable_registry_records.rs:158-166 can ever run.
4. Grep tests/ for the handler-authored sentences ("must be supplied", "filters may be applied"): no test covers them, confirming the four branches contribute no observable behaviour and only restate model numbers that can drift.
