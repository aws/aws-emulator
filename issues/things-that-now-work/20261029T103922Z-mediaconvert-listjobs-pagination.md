Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 6bb9010ea97ecc711aeaca7897e143f375b8cacc

# decode_cursor honours cursor spellings encode_cursor never writes, so a nextToken this service never issued is served as a page instead of refused

_Filed: 2026-10-29T10:39:22Z_
_Operation: ListJobs | Class: pagination_

> _Note: this issue was identified against the code base at commit `6bb9010ea97ecc711aeaca7897e143f375b8cacc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paged reader of this crate (ListJobs, SearchJobs, ListQueues, ListPresets, ListJobTemplates, ListVersions, StartJobsQuery) resolves its `nextToken` through the one shared rule `crate::state::resume_from_token` -> `crate::state::decode_cursor`. The crate states that rule as decode-or-refuse: a token this service never issued must be the modelled `BadRequestException`, "rather than an empty page or an arbitrary one".

But the reader is strictly more permissive than the one minting site. `encode_cursor` writes exactly `hex("<u128 decimal>:<id>")` — `format!("{}:{id}", epoch_nanos(created_at))`, so the instant is always a plain unsigned decimal with no sign and no leading zeros, and `id` is always a stored job id / queue name / preset name / template name, which is never empty (every minting door refuses a name that carries no characters). `decode_cursor` reads the instant with `nanos.parse().ok()?` at a `u128` target, and Rust's integer `FromStr` accepts an optional `+` sign and any number of leading zeros; `split_once(':')` also accepts an empty tail as the id.

So three families of token the service can never have issued decode successfully instead of being refused:

* `hex("+1700000000000000000:some-id")` — a `+`-signed instant,
* `hex("0000001700000000000000000:some-id")` — a leading-zero instant,
* `hex("1700000000000000000:")` — an empty id.

Each one is handed to `page_of_records` as a real anchor, which resumes "at the first record strictly past the anchor key in this direction". The caller therefore gets HTTP 200 with every record at or before that fabricated key silently hidden — the listing claims to be a page of a sequence the service never started — where the crate's own rule says the answer is 400. That is the decode-or-refuse rule failing open: an error turned into a success, and a page whose contents are silently truncated.

The crate's own token-refusal suite shows the intent and the hole in one place: it enumerates "the ways a token can fail to be one this service issued" — not hex, odd-length hex, no colon, a non-numeric instant, an instant past u128 — and has no row for a numeric instant spelled a way `encode_cursor` never spells it.

## Evidence

The ONE minting site, which never writes a sign, a leading zero or an empty id — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:1804-1817`:
```rust
/// The opaque `nextToken` of a paged listing: the hex of `<creation instant in epoch
/// nanoseconds>:<id>` — the ORDERING KEY of the last record of the page just returned.
...
/// Defined ONCE here, so every listing in this service issues and reads the same cursor
/// format.
pub fn encode_cursor(created_at: SystemTime, id: &str) -> String {
    ::hex::encode(format!("{}:{id}", epoch_nanos(created_at)))
}
```

The reader that accepts more than that — `src/state.rs:1819-1827`:
```rust
/// The `(creation instant, id)` ordering key inside a cursor, or `None` when the token is
/// not one [`encode_cursor`] produced — which a handler answers with its modelled
/// `BadRequestException`. Decoded with `ok()?` throughout, so a hostile token cannot
/// panic.
pub fn decode_cursor(token: &str) -> Option<(u128, String)> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (nanos, id) = decoded.split_once(':')?;
    Some((nanos.parse().ok()?, id.to_string()))
}
```
The doc comment itself states the contract the code does not keep: "`None` when the token is not one [`encode_cursor`] produced". `"+1700000000000000000".parse::<u128>()` and `"0001700000000000000000".parse::<u128>()` both succeed, so both are `Some(..)`.

The rule the refusal is supposed to implement — `src/state.rs:1848-1863`:
```rust
/// The decode-or-refuse rule, stated once: a token this service never issued names no page
/// to resume at, so it is a problem in the request rather than an empty page or an
/// arbitrary one. Each paged reader maps the refusal onto its own modelled
/// `BadRequestException`, which is the only part of it that differs between them.
pub fn resume_from_token(next_token: Option<&str>) -> Result<Option<(u128, String)>, String> {
```

What an accepted anchor then does — `src/state.rs:3554-3573`:
```rust
    let start = match resume_from {
        None => 0,
        Some((nanos, id)) => {
            let anchor = (*nanos, id.as_str());
            // Resume at the first record strictly past the anchor key in this direction.
```

The call site in the cited operation — `src/ops/list_jobs.rs:203-209`:
```rust
        let resume_from = crate::state::resume_from_token(input.next_token.as_deref()).map_err(
            |message| {
                ListJobsErrorModeled::BadRequestException(BadRequestException {
                    message: Some(message),
                })
            },
        )?;
```
The same two lines appear in `src/ops/search_jobs.rs:221-227`, `src/ops/list_queues.rs:184-190`, `src/ops/list_presets.rs:174-180`, `src/ops/list_job_templates.rs:183-189`, `src/ops/list_versions.rs:173-179` and `src/ops/start_jobs_query.rs:250-256`, so one fix covers all seven.

Test coverage, and the gap in it — `tests/all/shared_next_token_refusal.rs:244-262`:
```rust
/// Tokens this service did not issue: a token is the hex of `<epoch nanoseconds>:<id>`, so
/// each row breaks one part of that — the hex, its length, the colon, the number before it
/// — plus the degenerate empty one.
fn tokens_this_service_did_not_issue() -> Vec<(&'static str, String)> {
    vec![
        ("empty", String::new()),
        ("not hex at all", "zzzz".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of a string with no colon", ::hex::encode("no-colon")),
        (
            "hex of a non-numeric instant",
            ::hex::encode("not-a-number:some-record"),
        ),
        (
            "hex of an instant past what a count of nanoseconds can be",
            ::hex::encode(format!("{}0:some-record", u128::MAX)),
        ),
    ]
}
```
No row carries a `+`-signed instant, a leading-zero instant or an empty id, so none of the three spellings above is pinned either way; `tests/all/shared_paging.rs` exercises only tokens the service itself issued.

## Steps to Reproduce

1. Read `src/state.rs:1815-1817`: the one minting site writes `hex("<plain u128 decimal>:<non-empty id>")` and nothing else.
2. Read `src/state.rs:1823-1827`: the reader accepts the instant through `nanos.parse().ok()?` at a `u128` target and the id through `split_once(':')`, so `"+N"`, `"000N"` and an empty tail all decode.
3. Read `src/state.rs:1852-1855`: the stated rule is that a token this service never issued is "a problem in the request rather than an empty page or an arbitrary one".
4. Create 3 jobs, then call `GET /2017-08-29/jobs?nextToken=<hex of "+<nanos of the newest job>:x">` (hex-encode the ASCII string, e.g. `2b313730...`). Observe HTTP 200 with a silently truncated `jobs` list instead of the 400 `BadRequestException` the rule prescribes.
5. Repeat with `<hex of "0000<nanos>:x">` and with `<hex of "<nanos>:">` — both are accepted as page positions.
6. Compare `tests/all/shared_next_token_refusal.rs:244-262`: the suite enumerates the ways a token can fail to be one the service issued and has no row for any of these three spellings.
