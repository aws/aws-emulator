Issue-Source: trace-sweep-divergence
Issue-Trace: rbin-list-rules-pagination-and-filter
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rbin-list-rules-pagination-and-filter rbin-lock-conflict-tag-level-rule rbin-region-level-rule-with-exclude-tags rbin-tag-level-rule-lifecycle
Issue-Shape-Id: com.amazonaws.rbin#AmazonRecycleBin
Issue-Key: rbin|CreateRule|2xx->2xx|$.Tags
Issue-Commit: 0a2817577611e152839e8496ef0accae0bb6c9d7

# CreateRule response emits empty Tags array instead of omitting the unset member

> _Note: this issue was identified against the code base at commit `0a2817577611e152839e8496ef0accae0bb6c9d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRule

## Description
Cause 1 of 1: rbin CreateRule serializes the optional `Tags` output member as an empty JSON array when the created retention rule carries no tags, where AWS leaves the member out of the response body entirely.

AWS: Smithy model services/service-com-amazonaws-rbin-amazonrecyclebin/smithy-model/com-amazonaws-rbin-amazonrecyclebin.json - CreateRule (POST /rules, success 201), output shape CreateRuleResponse. Its `Tags` member targets `TagList` (list of `Tag`, @length min 0 max 200) and is NOT @required and carries NO @default. Under awsJson/restJson serialization an optional structure member with no @default and no value is simply not serialized, so a CreateRule call made without `Tags` (or with an empty `Tags`) returns a 201 body with no `Tags` key at all. The recorded AWS responses in the corpus confirm this: status 201 with `$.Tags` absent.

Emulator: replay of the same requests returns status 201 (status matches) but the body carries `"Tags": []`. The replay reports `$.Tags: type_mismatch (expected None, got [])` for every CreateRule step in the corpus (divergence_key `service=rbin|CreateRule|status=201->201|keys=$.Tags`), i.e. the serializer unconditionally materialises the list member from an empty in-memory collection rather than treating "no tags" as absent.

Why: clients distinguish an absent optional member from a present-but-empty one. A caller that tests for the presence of `Tags` to decide whether a rule was created with tags (or that round-trips a CreateRule response into an UpdateRule/TagResource call, or compares a CreateRule response against a GetRule/ListRules projection) sees a tag set that AWS never reported. Strict SDK/response-shape assertions and golden-body comparisons against real AWS break, and any logic branching on `Tags == null` versus `Tags.length == 0` takes the wrong branch. The same empty-collection-vs-absent bug class will affect the sibling optional list members of this response (`ResourceTags`, `ExcludeResourceTags`) and `LockConfiguration`/`LockState`, which AWS likewise omits when unset.

Fix: the rbin CreateRule response serializer (the component that builds the CreateRuleResponse JSON body from the stored rule). Optional, non-@default list members must be skipped when the stored value is absent/empty-because-unset rather than emitted as `[]`; the state representation needs to preserve "no Tags supplied" distinctly from "empty tag list" so the serializer can omit the key. No status code or error code change is involved - the status is already 201 and no error is in play.

Traces: rbin-list-rules-pagination-and-filter#setup#1 (representative), rbin-list-rules-pagination-and-filter#setup#2, rbin-lock-conflict-tag-level-rule#task#1, rbin-region-level-rule-with-exclude-tags#task#1, rbin-tag-level-rule-lifecycle#task#1 - all 5 CreateRule steps in the corpus diverge identically.

Note: the only other open rbin issue, 20261028T050716Z-rbin-getrule-maintainability.md (inlined `unanchored` pattern renderer in GetRule), is a maintainability item about a different operation and does not cover this response-shape divergence.

## Evidence
Replay verdict: mismatch. expected_status 201, actual_status 201. detail: `$.Tags: type_mismatch (expected None, got [])`. divergence_key: `service=rbin|CreateRule|status=201->201|keys=$.Tags`. affected_count: 5.

Expected (recorded AWS 201 body, shape only): CreateRuleResponse with `Identifier`, `RetentionPeriod`, `ResourceType`, `Status`, `RuleArn` and NO `Tags` key.
Actual (emulator 201 body): the same members plus `"Tags": []`.

Model evidence: CreateRuleResponse.Tags -> TagList, optional, no @default, @length(min 0, max 200); CreateRule http {method POST, uri /rules, code 201}; errors InternalServerException(500), ServiceQuotaExceededException(402), ValidationException(400) - none applicable here.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture `rbin-list-rules-pagination-and-filter`, step `setup#1`: a POST /rules (rbin CreateRule) whose request body supplies `RetentionPeriod` and `ResourceType` but no `Tags`.
3. Inspect the 201 response body. The emulator returns `"Tags": []`; the recorded AWS response has no `Tags` member.
4. Repeat with `rbin-tag-level-rule-lifecycle#task#1`, `rbin-region-level-rule-with-exclude-tags#task#1`, `rbin-lock-conflict-tag-level-rule#task#1` and `rbin-list-rules-pagination-and-filter#setup#2` for the same result.
