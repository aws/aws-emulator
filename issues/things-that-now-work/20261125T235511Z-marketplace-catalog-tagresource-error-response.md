Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 83705147667bf140869455e56830c0da9b266dd1

# Reserved-tag-key refusal splices the submitted key raw, so a key holding U+2028/U+2029 emits a multi-line Message

_Filed: 2026-11-25T23:55:11Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `83705147667bf140869455e56830c0da9b266dd1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every refusal this crate builds around a submitted value goes through `crate::state::quoting`, which renders the value single-line (`one_line`) before splicing it into the sentence, precisely because the member that carries the sentence (`ValidationException$Message` -> `ExceptionMessageContent`) declares `@pattern("^(.)+$")`, which matches no line terminator. The one refusal helper that does NOT do this is `crate::state::reserved_tag_key`: it interpolates the caller's tag key directly with `format!("tag key '{key}' ...")`.

`Tag$Key`'s modelled pattern is `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`. The Unicode `Z` category includes `Zl` (U+2028 LINE SEPARATOR) and `Zp` (U+2029 PARAGRAPH SEPARATOR), so the boundary validator accepts a key such as `aws:a\u{2028}b`. `is_reserved_tag_key` then matches its `aws:` prefix and the handler answers a `ValidationException` whose `Message` carries that raw line separator — a value `ExceptionMessageContent`'s `^(.)+$` forbids, and exactly the defect the crate's own `one_line` doc and the `single_line_refusal_messages` suite exist to prevent.

The same helper is reached from three handlers on inputs the model allows: `TagResource` (`Tags`), `UntagResource` (`TagKeys`) and `StartChangeSet` (`ChangeSetTags` and each change's `EntityTags`). The fix is one line — route the key through the existing `quoting` helper, as every sibling refusal already does.

## Evidence

Crate's own rule, state.rs:2717-2732 (`one_line`):
```
/// `text` as a single-line value carries it: every line terminator
/// ([`is_line_break`]) written as the escape sequence that stands for it ...
/// A sentence this service builds around a SUBMITTED value is carried by a
/// member whose shape declares `@pattern("^(.)+$")` —
/// `ExceptionMessageContent` on a `ValidationException$Message` ... — which
/// admits no line terminator, so a value carrying one is quoted ESCAPED
/// rather than spliced in raw.
fn one_line(text: &str) -> String {
```
and state.rs:2713-2714 counts U+2028/U+2029 as terminators: `matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')`.

The path every other refusal takes, state.rs:3518-3522:
```
fn quoting(prefix: &str, value: &str, suffix: &str) -> String {
    ...
    format!("{prefix}{}{suffix}", cut_to(&one_line(value), room, CUT_MARK))
```

The one that does not, state.rs:3537-3545:
```
pub fn reserved_tag_key(field: &str, key: &str) -> ValidationException {
    validation(
        field,
        format!(
            "tag key '{key}' begins with '{RESERVED_TAG_KEY_PREFIX}', the prefix reserved for \
             the tags Amazon Web Services creates, ..."
```

Callers: src/ops/tag_resource.rs:181-186 `if let Some(reserved) = crate::state::first_reserved_tag(&input.tags) { ... crate::state::reserved_tag_key("Tags", reserved)`; src/ops/untag_resource.rs:183-190 (`"TagKeys"`); src/ops/start_change_set.rs:219-226 (`"ChangeSetTags"`) and 310-319 (`"ChangeSet.EntityTags"`).

Model (smithy_query query="shape" shape="Tag,ExceptionMessageContent"): `Tag$Key` -> `"pattern": "^([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)$"`, `"length": {"min": 1, "max": 128}`; `ExceptionMessageContent` -> `"pattern": "^(.)+$"`, `"length": {"min": 1, "max": 2048}`. `Tag` carries no `skip_validation` (src/ops/types/shared_types.rs:128-133), so the boundary admits the key against that pattern and the handler builds the message.

Test coverage: tests/all/single_line_refusal_messages.rs:55-61 pins the rule and names the characters, including `("a line separator (U+2028)", '\u{2028}', r"\u2028")`; but its row table `cases()` (lines 178-265) exercises only `entityId`, `catalog`, `resourceArn`, `ClientRequestToken`, `Sort.SortBy` and a boundary-refused `FilterList.Name` — no row carries a line terminator in a tag KEY, so this path is the one the suite's rule does not reach.

## Steps to Reproduce

1. POST /TagResource with body {"ResourceArn": "arn:aws:aws-marketplace:us-east-1:000000000000:AWSMarketplace/AmiProduct/ent-000000000001", "Tags": [{"Key": "aws:a\u2028b", "Value": "v"}]} (the ARN only has to parse as a catalog resource of the emulator account — the reserved-key refusal is raised at src/ops/tag_resource.rs:181 before the store is read at line 194).
2. Read the 422 body: `Message` is "[Tags: tag key 'aws:a\u2028b' begins with 'aws:' ...]" with U+2028 spliced in RAW.
3. Compare with any sibling refusal that quotes a submitted value carrying the same character — e.g. GET /GetResourcePolicy?resourceArn=<value containing U+2028> — whose Message shows it escaped as \u2028 because it goes through `crate::state::quoting`.
4. Same result via POST /StartChangeSet with "ChangeSetTags": [{"Key": "aws:a\u2029b", "Value": "v"}] and via POST /UntagResource with "TagKeys": ["aws:a\u2028b"].
