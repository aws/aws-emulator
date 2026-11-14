Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0413f673f2bb729ba35510d6870123e90c83fff3

# Tag-key check accepts combining marks the modelled TagKey class refuses

_Filed: 2026-10-29T10:48:45Z_
_Operation: InvokeDataAutomationAsync | Class: validation_

> _Note: this issue was identified against the code base at commit `0413f673f2bb729ba35510d6870123e90c83fff3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::tag_key_is_well_formed` is the crate's own hand-rolled stand-in for the modelled `TagKey` `@pattern` (the framework cannot compile the pattern's `(?!aws:)` lookahead, so the handlers took enforcement over). Its doc claims the pattern is applied "verbatim", but the character-class half is applied as `char::is_alphabetic()`, which is the Unicode `Alphabetic` property — `\p{L}` PLUS `Other_Alphabetic`, i.e. combining marks (categories Mn/Mc). The modelled class names only `\p{L}`, `\p{Z}`, `\p{N}` and eight literal punctuation characters, so any key containing a combining mark (e.g. a Devanagari or Thai vowel sign, Mc/Mn, which ordinary Hindi/Thai text carries) is OUTSIDE the modelled pattern yet is accepted, persisted and reported back.

The code comment itself admits the divergence ("accepted where AWS's class would refuse it"), so the rule the function exists to enforce is knowingly not the rule it enforces. The stated reason for not applying the class with a regex — keeping "a second regex engine" out of the dependency graph — is contradicted by the framework this very comment cites: `aws-smithy-schema-server`, a direct dependency of this crate, already compiles every other `@pattern` in the same request walk with `regex::Regex`, so the engine is already in the graph.

Affected paths (all three tag-key accepting operations share the helper): `InvokeDataAutomationAsync` (`tags`), `TagResource` (`tags`), `UntagResource` (`tagKeys`). Expected: `ValidationException` (HTTP 400) in this service's envelope, the same answer the crate already gives for `100%done` or `team[eu]`. Actual: HTTP 200 and the key is stored/removed.

No test pins the accepting behaviour: the 14 rows of `tests/all/tag_key_refusal.rs` span letters, numbers, separators, symbols, control characters and ASCII punctuation, but none carries a combining mark.

## Evidence

Smithy model (`smithy_query` shape `TagKey`): `"pattern": "^(?!aws:)[\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*$"` — the class names only `\p{L}`, `\p{Z}`, `\p{N}` and `_.:/=+-@`.

src/state.rs:65-68 claims verbatim application:
```
/// The modelled `TagKey` pattern, verbatim (`smithy_query` shape `TagKey`).
///
/// Stated here because it has to be applied by hand: see [`tag_key_is_well_formed`].
pub const TAG_KEY_PATTERN: &str = r"^(?!aws:)[\p{L}\p{Z}\p{N}_.:/=+\-@]*$";
```
src/state.rs:108-110: "So the pattern is applied here, verbatim, by the three operations that accept a tag key (`InvokeDataAutomationAsync` and `TagResource` through `tags`, `UntagResource` through `tagKeys`)."

src/state.rs:88-92 admits it is NOT verbatim:
```
/// * `\p{L}` is `char::is_alphabetic` up to `Alphabetic`'s wider definition: that property
///   is `\p{L}` plus `Nl` — already admitted through `\p{N}` — plus `Other_Alphabetic`,
///   which is combining marks. A key carrying a combining mark is therefore accepted where
///   AWS's class would refuse it;
```
src/state.rs:95-100 is the code that does it:
```
fn is_tag_key_char(c: char) -> bool {
    c.is_alphabetic()
        || c.is_numeric()
        || (c.is_whitespace() && !c.is_control())
        || TAG_KEY_PUNCTUATION.contains(&c)
}
```
src/state.rs:117-118 states the rationale: "so the whole modelled constraint is enforced without a second regex engine in the dependency graph."

That rationale is contradicted inside the dependency it cites: vendor/smithy-schema-server/src/constraints.rs:418 `static REGEX_CACHE: LazyLock<Mutex<HashMap<&'static str, regex::Regex>>>` and :460 `let Ok(re) = regex::Regex::new(regex) else {` — the framework's own `@pattern` check — with vendor/smithy-schema-server/Cargo.toml:20 `regex = "1.12.3"`, and this crate depends on that framework at Cargo.toml:18 `aws-smithy-schema-server = { workspace = true }` (package `aws-emulator-smithy-schema-server`, vendor/smithy-schema-server/Cargo.toml:2). `regex` is in Cargo.lock:4991.

Test coverage: tests/all/tag_key_refusal.rs:197-244 enumerates 14 keys (`"請求書"`, `"share\u{bc}"`, `"cost\u{a0}center"`, `"cost\tcenter"`, `"100%done"`, `"team[eu]"`, `"budget\u{2264}cap"`, …) — no combining-mark row, so nothing pins the current acceptance.

## Steps to Reproduce

1. POST / with `X-Amz-Target: AmazonBedrockKeystoneRuntimeService.InvokeDataAutomationAsync`, a valid `inputConfiguration`/`outputConfiguration`/`dataAutomationProfileArn`, a resolvable `dataAutomationConfiguration.dataAutomationProjectArn`, and `tags: [{"key": "\u0915\u093e", "value": "v"}]` (Devanagari letter KA + vowel sign AA, the second character in category Mc).
2. Observe HTTP 200 and an `invocationArn`: `crate::state::malformed_tag_refusal` raised no refusal because `is_tag_key_char` accepted U+093E via `char::is_alphabetic()`, although `\p{L}`/`\p{Z}`/`\p{N}` do not contain category Mc.
3. Call `ListTagsForResource` with that ARN and see the key stored and reported back, where the modelled `TagKey` pattern required `ValidationException` (HTTP 400).
4. Same result through `TagResource` with that tag, and through `UntagResource` with `tagKeys: ["\u0915\u093e"]` — compare with key `"100%done"`, which the same helper refuses with `1 validation error detected: Value at 'tags.1.member.key' failed to satisfy constraint: Member must satisfy regular expression pattern: …`.
