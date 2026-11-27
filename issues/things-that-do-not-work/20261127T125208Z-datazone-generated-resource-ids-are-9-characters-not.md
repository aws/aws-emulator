Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: datazone.notebook-client-token-reuse-different-name-rejected datazone.notebook-update-then-delete-removes-access datazone.time-series-data-points-window-and-summary datazone.time-series-form-delete-scoped-to-its-name datazone.time-series-undeclared-content-member-rejected
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone.notebook-client-token-reuse-different-name-rejected|_shaped#2|AssertionError|assert isinstance(value, str) and pattern.fullmatch(value), f"{what} id has an unexpected shape: {value!r}"
Issue-Key: datazone.notebook-update-then-delete-removes-access|_shaped#2|AssertionError|assert isinstance(value, str) and pattern.fullmatch(value), f"{what} id has an unexpected shape: {value!r}"
Issue-Key: datazone.time-series-data-points-window-and-summary|_shaped#2|AssertionError|assert isinstance(value, str) and pattern.fullmatch(value), f"{what} id has an unexpected shape: {value!r}"
Issue-Key: datazone.time-series-form-delete-scoped-to-its-name|_shaped#2|AssertionError|assert isinstance(value, str) and pattern.fullmatch(value), f"{what} id has an unexpected shape: {value!r}"
Issue-Key: datazone.time-series-undeclared-content-member-rejected|_shaped#2|AssertionError|assert isinstance(value, str) and pattern.fullmatch(value), f"{what} id has an unexpected shape: {value!r}"
Issue-Commit: cd3ed932ac942c4bb826a6f713c43cf93575b0d9

# datazone: generated resource ids are 9 characters, not the 14 lowercase alphanumerics DataZone mints (CreateDomain/CreateProject/CreateAsset)

> _Note: this issue was identified against the code base at commit `cd3ed932ac942c4bb826a6f713c43cf93575b0d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Cause 1 of 1: every id this emulator mints for a DataZone resource is too short, so a client that recognises DataZone ids by their shape rejects them.

AWS: Amazon DataZone mints opaque resource ids as EXACTLY 14 lowercase-alphanumeric characters ([a-z0-9]{14}); a domain id is that same 14-character body behind a `dzd` prefix and a separator, e.g. the `dzd_<14 chars>` / `dzd-<14 chars>` form. Smithy only bounds the family and does not pin the length (CreateDomain.output.id -> DomainId, pattern ^dzd[-_][a-zA-Z0-9_-]{1,36}$; CreateProject/CreateAsset ids are plain generated ids), so the exact 14-character body is recorded behavior only: every replayed DataZone id in this suite has a 14-character body, and the checks encode it as `dzd[-_][a-z0-9]{14}` for a domain and `[a-z0-9]{14}` for a project/asset id. Only the LENGTH and character class are the requirement here — the particular characters of any one recorded id are a per-run value, not part of it.

Emulator: CreateDomain answers a domain id whose body is 9 characters, e.g. `dzd_mwyz4pn71` (prefix `dzd_` + the 9-character body `mwyz4pn71`); the body varies per run but its length does not. The id comes from `ComAmazonawsDatazoneDatazoneState::mint_id`, which concatenates base-36 of the mint instant in milliseconds (8 characters at current epoch values) with base-36 of a small per-process counter (1 character) — 9 characters in total, and it will drift to 10 only when the counter passes 35. The same mint feeds `rootDomainUnitId`, project ids, asset ids and the other generated ids in this service, so every one of them is short by the same construction. The five checks below all abort on the very first thing they assert about the response — `CreateDomain id has an unexpected shape: 'dzd_mwyz4pn71'` — so nothing further in them was exercised.

Why: a client that validates or routes on DataZone's id shape (a console-style regex, a parser that splits a fixed-width id, a fixture or type that declares the 14-character body, a test harness asserting the id it got back) refuses an id this emulator minted even though the call succeeded, and a caller that stores ids in a fixed-width column or derives keys from them gets values it cannot later match against real DataZone ids. Because the body is derived from the current clock, it is also not stable in width over time, so a consumer cannot even compensate with a looser fixed width.

Fix: the id mint in this service's state — `mint_id` in services/service-com-amazonaws-datazone-datazone/src/state.rs — must produce exactly 14 characters from the lowercase-alphanumeric class, keeping the uniqueness-within-a-process and never-reused properties it documents (e.g. pad/extend the encoded instant+counter to a fixed 14-character body) rather than letting the width follow base-36 of the clock. The `dzd` prefix and separator that CreateDomain prepends stay as they are; every caller of `mint_id` then inherits the corrected width, so no per-operation change is needed in the handlers.

Traces: the five datazone checks listed in trace_ids, each of which fails at its `_shaped(...)`/`_DOMAIN_ID` assertion on the CreateDomain response during setup. Citations: services/service-com-amazonaws-datazone-datazone/src/state.rs lines 2879-2889 (`mint_id`); services/service-com-amazonaws-datazone-datazone/src/ops/create_domain.rs line 316 (`format!("dzd_{}", store.mint_id(now))`) and line 324 (`root_domain_unit_id`).

## Evidence
Expected (AWS / replayed behavior): CreateDomain returns `id` matching `dzd[-_][a-z0-9]{14}` — a 14-character lowercase-alphanumeric body behind the `dzd` prefix; CreateProject and CreateAsset return ids matching `[a-z0-9]{14}`.

Actual (emulator): CreateDomain returns a 13-character id — a 9-character body — e.g. `dzd_mwyz4pn71`, `dzd_mwyzb2tm1`, `dzd_mwyz26ai1`, `dzd_mwyz1utg1`, `dzd_mwyz4rwg1` across the five replays (the body differs per run; its 9-character width does not). Harness output: `AssertionError: CreateDomain id has an unexpected shape: 'dzd_mwyz4pn71'`.

The status code and error channel are not involved — CreateDomain returns 201 and succeeds; only the minted id's shape diverges. `mint_id` = base36(now_ms) (8 chars) + base36(id_seq) (1 char) = 9 chars, versus the required 14.

## Steps to Reproduce
1. Boot the emulator and point a botocore `datazone` client at it.
2. Create an IAM role DataZone may assume, then call `create_domain(name=<unique>, domainExecutionRole=<role arn>, tags={...})`.
3. Read the returned `id`: it is `dzd_` plus a 9-character base-36 body (e.g. `dzd_mwyz4pn71`), so `re.fullmatch(r"dzd[-_][a-z0-9]{14}", id)` is None, while real DataZone ids match it.
4. Same for the ids returned by `create_project(domainIdentifier=...)` and `create_asset(...)`: `re.fullmatch(r"[a-z0-9]{14}", id)` is None.
5. Equivalently, replay any of the checks in trace_ids; each aborts in its setup at the CreateDomain id-shape assertion.
