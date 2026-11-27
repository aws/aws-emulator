Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 2554e5494fd859ae6eab8bc9ad0f2531c606d0f2

# The maxResults default-and-range rule is implemented twice, and the two copies have already drifted

_Filed: 2026-11-26T15:32:22Z_
_Operation: ListDiscoverableRegistryRecords | Class: maintainability_

> _Note: this issue was identified against the code base at commit `2554e5494fd859ae6eab8bc9ad0f2531c606d0f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Resolving a discovery read's page size is one rule — absent member becomes the documented default, a value outside the documented range becomes a 400 ValidationException naming `maxResults` and stating that range, otherwise the value is narrowed to `usize` — and it is written out twice in this crate, once in `ListDiscoverableRegistryRecords::handle` and once in `SearchDiscoverableRegistryRecords::handle`, with the identical refusal sentence spelled in both places. The two copies have ALREADY drifted in the narrowing step: the list copy falls back to `DEFAULT_MAX_RESULTS` if `usize::try_from` fails, the search copy falls back to `MAX_MAX_RESULTS as usize`, which are different numbers chosen for the same impossible branch. The crate already owns the shared home for this kind of refusal (`crate::state::field_validation`, which both copies call), so the sentence and the branch structure can live beside it once, parameterised by each operation's own `MIN_MAX_RESULTS` / `MAX_MAX_RESULTS` / `DEFAULT_MAX_RESULTS` constants, which stay where they are documented against each operation's model. Collapsing the two copies changes no response and no test: each operation keeps its own three constants and therefore its own numbers in the same sentence.

## Evidence

Copy 1 — services/service-com-amazonaws-agentregistry-agentregistry/src/ops/list_discoverable_registry_records.rs:204-217:
```
        let max_results = match input.max_results {
            Some(requested) => {
                if !(MIN_MAX_RESULTS..=MAX_MAX_RESULTS).contains(&requested) {
                    return Err(field_validation(
                        "maxResults",
                        &format!(
                            "{requested} is outside the valid range {MIN_MAX_RESULTS} through {MAX_MAX_RESULTS}."
                        ),
                    ));
                }
                usize::try_from(requested).unwrap_or(DEFAULT_MAX_RESULTS)
            }
            None => DEFAULT_MAX_RESULTS,
        };
```

Copy 2 — services/service-com-amazonaws-agentregistry-agentregistry/src/ops/search_discoverable_registry_records.rs:182-195 — the same branch and the same sentence, character for character:
```
        let requested = if input.max_results == 0 {
            DEFAULT_MAX_RESULTS
        } else {
            input.max_results
        };
        if !(MIN_MAX_RESULTS..=MAX_MAX_RESULTS).contains(&requested) {
            return Err(field_validation(
                "maxResults",
                &format!(
                    "{requested} is outside the valid range {MIN_MAX_RESULTS} through {MAX_MAX_RESULTS}."
                ),
            ));
        }
        let max_results = usize::try_from(requested).unwrap_or(MAX_MAX_RESULTS as usize);
```

The drift that already exists between them: `unwrap_or(DEFAULT_MAX_RESULTS)` (list_discoverable_registry_records.rs:214) against `unwrap_or(MAX_MAX_RESULTS as usize)` (search_discoverable_registry_records.rs:195) for the same unreachable conversion failure.

The shared home both copies already reach for the refusal itself — services/service-com-amazonaws-agentregistry-agentregistry/src/state.rs:113-117:
```
/// The modelled `ValidationException` naming the member that failed and why.
///
/// Shared by every operation so one refusal is worded one way; each operation
/// lifts it into its own error channel with `.into()`.
pub fn field_validation(member: &str, message: &str) -> ValidationException {
```

Tests that pin the behaviour this must not change, services/service-com-amazonaws-agentregistry-agentregistry/tests/all/discovery_read_path.rs:
- the refusal table rows `("maxResults above the documented list maximum", ... json!({ "maxResults": 101 }))` (:1254-1256), `("maxResults explicitly zero (list)", ... json!({ "maxResults": 0 }))` (:1259-1261), `("maxResults above the documented search maximum", ... "maxResults": 21)` (:1170-1172), `("maxResults explicitly zero (search)", ... "maxResults": 0)` (:1175-1177), `("maxResults below the documented search minimum", ... "maxResults": -1)` (:1180-1182);
- `list_pages_through_exactly_the_discoverable_records_at_every_page_size` (:450) over `maxResults` 1,2,3,4,100 and absent (:455-461);
- `search_truncates_to_max_results_defaulting_to_ten` (:1038) including the absent row `// Absent: the model declares `@default: 10`.` / `(None, 10)` (:1047-1048).

## Steps to Reproduce

1. Open services/service-com-amazonaws-agentregistry-agentregistry/src/ops/list_discoverable_registry_records.rs and read lines 204-217: the page-size rule (default when absent, 400 ValidationException naming `maxResults` with the sentence "{requested} is outside the valid range {MIN} through {MAX}.", then narrowing to usize).
2. Open services/service-com-amazonaws-agentregistry-agentregistry/src/ops/search_discoverable_registry_records.rs and read lines 182-195: the same three steps and the identical sentence, differing only in the operation's own constants.
3. Compare the narrowing fallbacks: list_discoverable_registry_records.rs:214 uses `unwrap_or(DEFAULT_MAX_RESULTS)` while search_discoverable_registry_records.rs:195 uses `unwrap_or(MAX_MAX_RESULTS as usize)` — the copies have already drifted on the same branch.
4. Observe that a change to the refusal wording or to the absent-member handling must now be made in two files for the two reads to keep answering alike; nothing in either file points at the other.
