Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4c2ddda98f023f570764c4b2cc687bfb0de98717

# StartSnapshot accepts and persists any number of Tags, past the documented 50 per snapshot

_Filed: 2026-11-26T18:03:52Z_
_Operation: StartSnapshot | Class: validation_

> _Note: this issue was identified against the code base at commit `4c2ddda98f023f570764c4b2cc687bfb0de98717`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartSnapshot` copies the request's `Tags` list into the stored snapshot without ever judging how many tags it carries, and then echoes every one of them back. The model documents the limit explicitly — "You can specify up to 50 tags per snapshot" — and the handler's own module doc carries that same sentence (it is the member doc the scaffold copied in), so the rule is stated directly above the code that ignores it.

Nothing enforces it anywhere else either: `com.amazonaws.ebs#Tags` is a bare `list` of `Tag` with no `@length`, so the request boundary applies no bound, and the handler's only `Tags` handling is the `map` into `StoredTag`. A request carrying 51 — or 5,000 — tags is answered 201 and the whole list is persisted on the record and reported in the response body, while the model keeps a dedicated `ValidationExceptionReason` value for a bad tag (`INVALID_TAG`) that this crate never produces.

No comment, constant or test in the crate justifies the divergence: the only tag-related text in `src/` is the `StoredTag` struct ("Mirrors the modeled `Tag` shape, whose members are both optional"), and the tests exercise at most three tags, so no test pins an unbounded list.

## Evidence

MODEL — `smithy_query(query="doc", shape="com.amazonaws.ebs#StartSnapshotResponse", member="Tags")`:
"The tags applied to the snapshot. You can specify up to 50 tags per snapshot. For more information, see Tagging your Amazon EC2 resources in the Amazon Elastic Compute Cloud User Guide."

MODEL — `smithy_query(query="shape", shape="com.amazonaws.ebs#Tags")` carries no bound at all, so the boundary enforces none:
{"shape_id": "com.amazonaws.ebs#Tags", "type": "list", "member": "Tag"}

MODEL — `smithy_query(query="shape", shape="com.amazonaws.ebs#ValidationExceptionReason")` declares a reason for exactly this class of refusal, which the crate never constructs:
"values": ["INVALID_CUSTOMER_KEY", "INVALID_PAGE_TOKEN", "INVALID_BLOCK_TOKEN", "INVALID_GRANT_TOKEN", "INVALID_SNAPSHOT_ID", "UNRELATED_SNAPSHOTS", "INVALID_BLOCK", "INVALID_CONTENT_ENCODING", "INVALID_TAG", "INVALID_DEPENDENCY_REQUEST", "INVALID_PARAMETER_VALUE", "INVALID_VOLUME_SIZE", "CONFLICTING_BLOCK_UPDATE", "INVALID_IMAGE_ID", "WRITE_REQUEST_TIMEOUT"]

CODE — the handler's OWN doc states the rule, services/service-com-amazonaws-ebs-ebs/src/ops/start_snapshot.rs:120-122:
"//! * `Tags` (Tags) — The tags applied to the snapshot. You can specify up to 50 tags per
//!   snapshot. For more information, see Tagging your Amazon EC2 resources in the Amazon
//!   Elastic Compute Cloud User Guide."

CODE — and the code under it applies no count check, services/service-com-amazonaws-ebs-ebs/src/ops/start_snapshot.rs:444-453:
"            tags: input
                .tags
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|tag| StoredTag {
                    key: tag.key,
                    value: tag.value,
                })
                .collect(),"

CODE — every other documented-but-unmodelled bound of this same operation IS enforced by the handler, which is the pattern this one departs from, services/service-com-amazonaws-ebs-ebs/src/ops/start_snapshot.rs:256-264:
"        // `VolumeSize`: \"The maximum size is 65536 GiB (64 TiB).\" The model carries only
        // the minimum, so the documented maximum is refused here, with the service's own
        // request-derived template for the refusal.
        if input.volume_size > MAX_VOLUME_SIZE_GIB {
            return Err(validation(
                \"INVALID_VOLUME_SIZE\",
                invalid_volume_size_message(input.volume_size),
            ));
        }"
with the matching constant services/service-com-amazonaws-ebs-ebs/src/state.rs:29-32: "/// `StartSnapshotRequest$VolumeSize` documents \"The maximum size is 65536 GiB (64 TiB)\"; the model's own `range` only carries the minimum, so the maximum is enforced by the handler."

CODE — the store keeps the list unbounded, services/service-com-amazonaws-ebs-ebs/src/state.rs:292: "    pub tags: Vec<StoredTag>,"

TEST COVERAGE — no test pins an unbounded tag list; the largest row anywhere is three tags (tests/all/snapshot_block_loop.rs:748-752 "three tags and an explicit timeout", tests/all/response_bodies_and_refusals.rs:103-106 "unencrypted with two tags"), and `grep -n "[Tt]ag" src/` finds no comment stating a reason for accepting more.

## Steps to Reproduce

1. POST /snapshots with body {"VolumeSize": 1, "Tags": [ ... 51 distinct {"Key","Value"} entries ... ]}.
2. Observe HTTP 201: the snapshot is created and the response body's Tags array carries all 51 entries, although the modelled member documents "You can specify up to 50 tags per snapshot" and the model declares an INVALID_TAG ValidationExceptionReason.
3. Repeat with 5000 tags: still 201, and every tag is persisted on the stored record (`Snapshot::tags`), so the oversized list survives into every later reader of the store.
