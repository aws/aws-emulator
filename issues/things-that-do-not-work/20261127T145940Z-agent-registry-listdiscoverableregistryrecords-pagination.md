Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 680677ada8878ce293f02ee08799bbda12b1d232

# The List resumption comment denies the record-id ordering its own token scheme is built on, contradicting the state module that declares that ordering load-bearing

_Filed: 2026-11-27T14:59:40Z_
_Operation: ListDiscoverableRegistryRecords | Class: pagination_

> _Note: this issue was identified against the code base at commit `680677ada8878ce293f02ee08799bbda12b1d232`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListDiscoverableRegistryRecords` paginates by minting a token from `last_record_id` — the record id of the LAST summary pushed, i.e. the last record in iteration order — and resumes with the predicate `record.record_id.as_str() > after`. That scheme is correct only while iteration visits records in ascending record-id order: `after` is the last record *seen*, not the maximum id of the page, so under any other iteration order the resume predicate would drop records that were never returned (every matching record whose id sorts below `after` but which was not reached before the page filled) and could return records a second time (every record already on the page whose id sorts above `after`).

The handler's own comment above that predicate asserts the opposite invariant: that the `filter` form means "resumption does not depend on the iteration being ordered". The crate's state module states the true invariant explicitly — the record map is a `BTreeMap` *because* "iteration order is stable and a pagination token can resume from a record id", and `discoverable_records()` is documented as yielding records "in record-id order". So one comment in this crate declares the ordering to be exactly what makes the token work, and the other declares the token independent of it.

There is no behavioural divergence today (the `BTreeMap` supplies the ordering), which is precisely why the false invariant is the defect worth recording: it tells a future maintainer that `Registry::records` may be swapped for an unordered container (a `HashMap`, a `Vec` in insertion order) without touching the pagination code, and that change would silently skip and duplicate records across pages while every existing paging test still has to be re-derived to notice. The claim stated above the predicate is not true of the code under it.

## Evidence

The claim, services/service-com-amazonaws-agentregistry-agentregistry/src/ops/list_discoverable_registry_records.rs:251-258:
```
            // Resume strictly after the record the previous page stopped on. A
            // `filter` rather than a `skip_while`, so resumption does not depend on
            // the iteration being ordered.
            .filter(|record| {
                start_after
                    .as_deref()
                    .is_none_or(|after| record.record_id.as_str() > after)
            });
```

The code that does depend on the ordering — `after` is whatever record was visited LAST, not the page's maximum id — same file :260-273:
```
        let mut registry_records = Vec::new();
        let mut next_token = None;
        let mut last_record_id: Option<&str> = None;
        for record in &mut matching {
            if registry_records.len() == max_results {
                // At least one more record matches, so this page is truncated and the
                // caller gets a token that resumes after the last one returned.
                next_token = last_record_id
                    .map(|last| encode_next_token(&registry.registry_id, last));
                break;
            }
            registry_records.push(record.discoverable_summary());
            last_record_id = Some(record.record_id.as_str());
        }
```
and the handler's own promise about that token, same file :229-230:
```
        // Resume exactly after the record the previous page stopped on, so no record
        // is repeated or skipped. A token minted for another registry is refused.
```

The other side of the contradiction — this crate's state module, which declares the ordering to be the reason the token scheme works, services/service-com-amazonaws-agentregistry-agentregistry/src/state.rs:329-336:
```
/// One registry and the records it owns, keyed by record id so iteration order is
/// stable and a pagination token can resume from a record id.
#[derive(Debug, Clone)]
pub struct Registry {
    pub registry_id: String,
    pub registry_arn: String,
    pub records: BTreeMap<String, RegistryRecord>,
}
```
and src/state.rs:339-344:
```
    /// This registry's discoverable records, in record-id order.
    pub fn discoverable_records(&self) -> impl Iterator<Item = &RegistryRecord> {
        self.records
            .values()
            .filter(|record| record.is_discoverable())
    }
```

Test coverage: the paging walk `list_every_page` (services/service-com-amazonaws-agentregistry-agentregistry/tests/all/discovery_read_path.rs:461-493) and `list_pages_through_exactly_the_discoverable_records_at_every_page_size` (:496) assert the full multi-page result set at every page size, and the suite's record list is declared in id order — `const DISCOVERABLE: [&str; 3] = ["rec000000001", "rec000000002", "rec000000004"];` with the comment "the discoverable (`APPROVED`) records of [`REGISTRY`], in the record-id order every read reports them in" (discovery_read_path.rs:47-49). Those tests pass only because the `BTreeMap` supplies the ordering the handler comment says is not needed; no test exercises the handler against an unordered record iteration.

## Steps to Reproduce

1. Read services/service-com-amazonaws-agentregistry-agentregistry/src/ops/list_discoverable_registry_records.rs:251-253 — the comment states that using a `filter` rather than a `skip_while` means "resumption does not depend on the iteration being ordered".
2. Read the token mint 13 lines below it (:267-272): `last_record_id` is assigned on every pushed record, so after the loop it holds the id of the record visited LAST, and the token encodes that id; the resume predicate at :254-258 then keeps only records whose id sorts strictly above it.
3. Read services/service-com-amazonaws-agentregistry-agentregistry/src/state.rs:329-330 and :339 — the record map is a `BTreeMap` precisely so that "iteration order is stable and a pagination token can resume from a record id", and `discoverable_records()` yields records "in record-id order". The two comments make opposite claims about the same mechanism.
4. Confirm the dependency the handler comment denies: with `Registry::records` iterated in any order other than ascending record id, a page of `maxResults` records whose last-visited id is not the page maximum makes the next page's predicate `record_id > after` exclude every still-unreturned record whose id sorts below `after` (records silently skipped) and include page-one records whose id sorts above it (records returned twice) — breaking the handler's own guarantee at :229-230 that "no record is repeated or skipped".
5. Note that tests/all/discovery_read_path.rs:461-493 and :496 assert full-result-set paging only against the ordered `BTreeMap`, so nothing in the crate would flag the regression the false invariant invites.
