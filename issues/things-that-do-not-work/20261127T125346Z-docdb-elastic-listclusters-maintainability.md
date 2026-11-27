Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# The 'decode a nextToken or refuse it' rule is implemented three times, once per list handler

_Filed: 2026-11-27T12:53:45Z_
_Operation: ListClusters | Class: maintainability_

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a `nextToken` this service did not issue is a `ValidationException` whose message is `NextToken must be a pagination token this service issued.`, otherwise it is the inclusive start key of the page" is written out three times — once in each list handler — together with the follow-up conversion of that start key into a `std::ops::Bound`. `state.rs` factors out only the codec half (`encode_page_token` / `decode_page_token`); the decide-and-refuse half, including the refusal sentence and the `Bound` construction, is copied per handler, so the three can drift (one could start accepting a foreign token, or word the refusal differently) while the suites that pin each one pass independently.

One helper in `state.rs` beside `decode_page_token` — taking `Option<&str>` and answering the `Bound` or the `ValidationException` — collapses all three copies. Every response body, status and existing assertion stays byte-identical: the handlers already produce the same sentence and the same start bound.

## Evidence

Copy 1 — `services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/list_clusters.rs:160-170,181-184`
```
        // The cursor names the key the next page starts at. A token this service did
        // not issue is a bad request, not an empty page.
        let start = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(state::decode_page_token(token).ok_or_else(|| {
                ListClustersErrorModeled::ValidationException(state::invalid_input(
                    token,
                    "NextToken must be a pagination token this service issued.",
                ))
            })?),
        };
...
        let first = match start {
            Some(key) => std::ops::Bound::Included(key),
            None => std::ops::Bound::Unbounded,
        };
```

Copy 2 — `services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/list_cluster_snapshots.rs:180-190,199-202`
```
        // The cursor names the key the next page starts at. A token this service did
        // not issue is a bad request, not an empty page.
        let start = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(state::decode_page_token(token).ok_or_else(|| {
                ListClusterSnapshotsErrorModeled::ValidationException(state::invalid_input(
                    token,
                    "NextToken must be a pagination token this service issued.",
                ))
            })?),
        };
...
        let first = match start {
            Some(key) => std::ops::Bound::Included(key),
            None => std::ops::Bound::Unbounded,
        };
```

Copy 3 — `services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/list_pending_maintenance_actions.rs:157-169,177-180`
```
        // The cursor names the key the next page starts at. A token this service did
        // not issue is a bad request, not an empty page.
        let start = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(state::decode_page_token(token).ok_or_else(|| {
                ListPendingMaintenanceActionsErrorModeled::ValidationException(
                    state::invalid_input(
                        token,
                        "NextToken must be a pagination token this service issued.",
                    ),
                )
            })?),
        };
...
        let first = match start {
            Some(key) => std::ops::Bound::Included(key),
            None => std::ops::Bound::Unbounded,
        };
```

The codec the three share is already factored out, so the duplicated part is the decision, not the encoding — `services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/state.rs:632-636`:
```
/// Decode a cursor produced by [`encode_page_token`]; `None` when it is not one.
pub fn decode_page_token(token: &str) -> Option<String> {
```

Tests that pin all three copies (they must keep passing unchanged):
* `tests/all/cluster_lifecycle.rs:653-659` — "case {case:?}: a foreign nextToken must be refused" for `GET /clusters?nextToken=...`;
* `tests/all/cluster_snapshot_listing.rs:523-532` — the same for `GET /cluster-snapshots?nextToken=...`;
* `tests/all/maintenance_and_restore.rs:562-576` — "a forged pagination token is HTTP 400" / `ValidationException` for `GET /pending-actions?nextToken=...`;
* plus `tests/all/refusal_wording.rs:335-337`, `tests/all/error_type_header_form.rs:322-324` and `tests/all/response_media_type.rs:240` which assert the wording, header and media type of that same refusal.

## Steps to Reproduce

1. Open the three list handlers side by side: src/ops/list_clusters.rs:160-170, src/ops/list_cluster_snapshots.rs:180-190, src/ops/list_pending_maintenance_actions.rs:157-169.
2. Observe the identical decode-or-refuse branch and refusal sentence, differing only in the generated error-enum name, followed by the identical `Bound::Included`/`Bound::Unbounded` conversion in each.
3. Change the sentence in one copy only and note that the other two suites still pass, which is the drift this duplication allows.
