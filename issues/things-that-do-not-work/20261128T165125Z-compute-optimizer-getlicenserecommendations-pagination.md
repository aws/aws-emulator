Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: b60f6fc1d7d48b8ab01ee41fe8076d8fc438ec86

# decode_offset_page_token honours offsets encode_offset_page_token never writes, so a tampered nextToken resumes a walk — and a past-the-end offset is answered with the empty final page the crate's other readers refuse

_Filed: 2026-11-28T16:51:25Z_
_Operation: GetLicenseRecommendations | Class: pagination_

> _Note: this issue was identified against the code base at commit `b60f6fc1d7d48b8ab01ee41fe8076d8fc438ec86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The five offset-paged readers of this service (GetLicenseRecommendations, GetIdleRecommendations, GetRDSDatabaseRecommendations, GetEnrollmentStatusesForOrganization, DescribeRecommendationExportJobs) enter their walk through `crate::state::decode_offset_page_token`, whose own doc says it answers "the cursor a `nextToken` this service issued stands for, or `None` for a token it did not issue". The token is plain `hex` of `"<offset>:<fingerprint>"` with no integrity value, and the decoder's ONLY check on the offset half is a bare `offset.parse().ok()?`.

Two consequences, both of them tokens this service never minted being served as page positions:

1. Non-canonical spellings. `encode_offset_page_token` renders the offset through `usize`'s `Display`, which emits exactly one decimal form, but `usize::from_str` also accepts a leading `+` and leading zeros, so hex of `"007:<fingerprint>"` or `"+7:<fingerprint>"` decodes to the same cursor and resumes the walk although the service can never have written it.
2. A substituted offset. Because the token carries the cursor in the clear and the fingerprint is copied verbatim from an issued token, a client can hex-decode a legitimate token, replace the offset with any number, re-encode, and the reader honours it: the fingerprint comparison the handlers make (`cursor.selection_fingerprint == selection_fingerprint`) still passes. An offset past the end of the selected sequence then falls through `offset_page`'s `skip(offset)` to an EMPTY page with NO `nextToken` — a 200 a client reads as "the result set ended here".

That last outcome is exactly what this crate's boundary-token readers refuse, for the reason they state in their own comments: a token that names no position in the current result set is refused "rather than answered with an empty final page a caller would read as 'no more recommendations'". So the two halves of this one service disagree about what to do with a cursor that names no boundary of the selection: the ARN/key-boundary readers refuse it, the offset readers serve an authoritative-looking end of the list.

## Evidence

The only offset spelling the service ever mints — services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/src/state.rs:2301-2308:
```
/// The opaque `nextToken` an offset-paged reader hands out: the cursor itself, hex-encoded,
/// so a page token needs no server-side entry and a read never writes. ...
pub fn encode_offset_page_token(cursor: &PageCursor) -> String {
    ::hex::encode(format!("{}:{}", cursor.offset, cursor.selection_fingerprint))
}
```
(`PageCursor::offset` is a `usize` — src/state.rs:2275 — so `Display` renders one canonical decimal.)

The decoder's stated rule and the parse that breaks it — src/state.rs:2310-2320:
```
/// The cursor a `nextToken` this service issued stands for, or `None` for a token it did
/// not issue. What a decoded cursor may resume is the caller's own check: it compares the
/// cursor's fingerprint against the one it would issue for this request.
pub fn decode_offset_page_token(token: &str) -> Option<PageCursor> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let (offset, selection_fingerprint) = decoded.split_once(':')?;
    Some(PageCursor {
        offset: offset.parse().ok()?,
        selection_fingerprint: selection_fingerprint.to_string(),
    })
}
```

The handler that takes the decoded offset at its word — src/ops/get_license_recommendations.rs:257-269:
```
        let offset = match input.next_token.as_deref() {
            None => 0,
            Some(token) => {
                let cursor = crate::state::decode_offset_page_token(token)
                    .filter(|cursor| cursor.selection_fingerprint == selection_fingerprint);
                let Some(cursor) = cursor else {
                    return Err(invalid_parameter(
                        crate::state::INVALID_NEXT_TOKEN_MESSAGE.to_string(),
                    ));
                };
                cursor.offset
            }
        };
```
and the same branch at src/ops/get_idle_recommendations.rs:289-302, src/ops/get_rds_database_recommendations.rs:298-310, src/ops/get_enrollment_statuses_for_organization.rs:203-215, src/ops/describe_recommendation_export_jobs.rs:227-239.

An out-of-range offset is served, not refused — src/state.rs:2351-2371:
```
pub fn offset_page<T>(
    selected: Vec<T>,
    offset: usize,
    page_size: usize,
    selection_fingerprint: String,
) -> (Vec<T>, Option<String>) {
    let mut page: Vec<T> = selected
        .into_iter()
        .skip(offset)
        .take(page_size.saturating_add(1))
        .collect();
```

The rule the crate's OTHER readers apply to a cursor that names no boundary of the current selection — src/ops/get_recommendation_preferences.rs:264-275:
```
        // Where this page starts: the position the record a `nextToken` named holds in that
        // sequence. A token whose record the sequence no longer holds — it was deleted — names
        // no boundary of THIS result set, so it is refused rather than answered with an empty
        // final page a caller would read as "no more preferences"
        // ([`crate::state::resume_index`]).
```
and src/ops/get_ec2_instance_recommendations.rs:331-347 ("it is refused rather than answered with an empty final page a caller would read as 'no more recommendations'").

Test coverage leaves the offset half unpinned — tests/all/shared_paging_rules.rs:432:
```
const UNRESUMABLE_TOKENS: &[&str] = &["not-a-token", "", "7b7d", "deadbeef", "0"];
```
no row spells an offset non-canonically and no row keeps a valid fingerprint while changing the offset (tests/all/shared_paging_rules.rs:600-651 only varies the SELECTION).

## Steps to Reproduce

1. Opt the account in (UpdateEnrollmentStatus status=Active) and let the analysis become due so GetLicenseRecommendations reports at least two license recommendations.
2. Call GetLicenseRecommendations with {"maxResults": 1} — 200, one recommendation and a nextToken T.
3. Hex-decode T: it reads "1:<fingerprint>" (src/state.rs:2306).
4. Re-encode the SAME cursor with a non-canonical offset — hex of "01:<fingerprint>" (or "+1:<fingerprint>") — and call GetLicenseRecommendations with {"maxResults": 1, "nextToken": <that hex>}. Observe 200 and the second page: `offset.parse()` (src/state.rs:2317) accepted a spelling `format!("{}", usize)` can never emit, so a token this service never minted resumed the walk. Expected the refusal every unissued token gets (400 InvalidParameterValueException, crate::state::INVALID_NEXT_TOKEN_MESSAGE).
5.  Re-encode with an offset past the end — hex of "999:<fingerprint>" — and call again. Observe 200 with an empty licenseRecommendations list and no nextToken, i.e. an authoritative-looking end of the result set, although no page ever minted offset 999.
6. Repeat step 4/5 against GetIdleRecommendations, GetRDSDatabaseRecommendations, GetEnrollmentStatusesForOrganization or DescribeRecommendationExportJobs with that reader's own first-page token: the same tampered offsets are honoured, because the defect is in the shared crate::state::decode_offset_page_token.
