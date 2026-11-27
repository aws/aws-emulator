Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ea455e680b1a60e9de168b48445650a7c0dba68e

# GetImages accepts a ListFragments-minted NextToken and answers a hollow 200 with an empty Images list

_Filed: 2026-11-27T00:29:22Z_
_Operation: GetImages | Class: validation_

> _Note: this issue was identified against the code base at commit `ea455e680b1a60e9de168b48445650a7c0dba68e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`FragmentCursor` is a single, untagged pagination cursor shared by `ListFragments` and `GetImages`, and the `interval_millis` member encodes BOTH "a fragment listing" and "an image page whose query named no interval" as `None`. So a `NextToken` minted by `ListFragments` is indistinguishable from a `GetImages` token for a query that named no `SamplingInterval`.

`GetImages` therefore ACCEPTS a `ListFragments` continuation token whenever the request repeats the same stream, selector origin and window and names no `SamplingInterval`: all four equality checks pass and `cursor.interval_millis` is `None`, which the handler then treats as "continue on the interval the token carries". The offset in that token counts ARCHIVED FRAGMENTS, but `GetImages` applies it as an index into the sampled-instant list, which for an absent interval holds exactly ONE instant (the window's start). `skip(offset)` with `offset >= 1` yields nothing, `next_offset < instants.len()` is false, and the handler answers HTTP 200 with `Images: []` and no `NextToken` — a success claiming the window has no images at all, for a window the very same handler just proved holds media (it passed `holds_media_in_range`, otherwise it would have refused). The documented refusal for a token that names another result set (`InvalidArgumentException`, which this handler already sends for a cursor whose stream/window disagree) is never reached.

This is the reverse direction of the already-open `ListFragments` cursor issues: there the foreign token mis-offsets a real list; here it turns a populated window into an empty, terminal page.

## Evidence

The one untagged cursor, `src/state.rs:1226-1243`:
```
pub struct FragmentCursor {
    pub stream_name: String,
    pub origin: TimestampOrigin,
    pub start_millis: i64,
    pub end_millis: i64,
    pub offset: usize,
    /// The sampling interval, in milliseconds, the instants this cursor's offset
    /// indexes into were computed at, when the paged result set is a sampled one.
    /// [`None`] for a fragment listing, whose results the archive itself
    /// enumerates, and for an image page whose query named no interval.
    pub interval_millis: Option<i64>,
}
```
— the doc itself states the two different result sets share the `None` encoding.

`ListFragments` mints exactly that cursor, `src/ops/list_fragments.rs:315-324` and `:350-357`:
```
                FragmentCursor {
                    stream_name: stream.name.clone(),
                    origin,
                    start_millis,
                    end_millis,
                    offset: 0,
                    // A fragment listing's results are the archive's own, so no
                    // sampling shapes the set this cursor walks.
                    interval_millis: None,
                }
```
```
        let next_offset = cursor.offset.saturating_add(fragments.len());
        let next_token = (next_offset < matching.len()).then(|| {
            FragmentCursor { offset: next_offset, ..cursor }.encode()
        });
```
(the offset counts FRAGMENTS of `matching`).

`GetImages` accepts it, `src/ops/get_images.rs:517-536`:
```
                    let minted = FragmentCursor::decode(token).filter(|cursor| {
                        cursor
                            .interval_millis
                            .is_none_or(|millis| millis >= MIN_SAMPLING_INTERVAL_MILLIS)
                    });
                    ...
                    if cursor.stream_name != stream.name
                        || cursor.origin != origin
                        || cursor.start_millis != start_millis
                        || cursor.end_millis != end_millis
                        || (requested_interval.is_some()
                            && requested_interval != cursor.interval_millis)
                    {
                        return Err(invalid(TOKEN_FOR_ANOTHER_QUERY_MESSAGE.to_string()));
                    }
                    (cursor.offset, cursor.interval_millis)
```
Nothing distinguishes the cursor's provenance, and with `requested_interval == None` the interval clause is skipped.

The offset is then used as an index into the sampled instants, `src/ops/get_images.rs:558` and `:573-581`:
```
            let instants = sampled_instants(start_millis, end_millis, interval);
```
```
            let sampled: Vec<SampledInstant> = instants
                .iter()
                .skip(offset)
                .take(page_size)
```
and with no interval there is exactly one instant, `src/ops/get_images.rs:691-697`:
```
fn sampled_instants(start_millis: i64, end_millis: i64, interval: Option<i64>) -> Vec<i64> {
    let Some(interval) = interval.filter(|&interval| interval > 0) else {
        return vec![start_millis];
    };
```
so `skip(offset >= 1)` empties the page, and the answer is a success, `src/ops/get_images.rs:583-585` / `:628-631`:
```
            let next_offset = offset.saturating_add(sampled.len());
            let next_token = (next_offset < instants.len()).then(|| { ... });
```
```
        Ok(GetImagesOutput {
            images: Some(images),
            next_token,
        })
```

The handler's own stated rule for a foreign token, `src/ops/get_images.rs:231-237`, is the one it fails to apply here: "The token ... is issued for ONE listing ... the stream, the selector, the window and the sampling interval it was minted over are what a continuation must agree with" — a fragment listing is not a listing of sampled instants at all.

Test coverage: `tests/all/get_images_sampling.rs` and `tests/all/get_images_generation.rs` exercise GetImages tokens minted by GetImages only; no test replays a `ListFragments` token against `/getImages`.

## Steps to Reproduce

1. Seed one ACTIVE stream with retention > 0 and three archived fragments whose producer timestamps are T0, T0+10s and T0+20s (each with real media).
2. POST /listFragments with {StreamName, MaxResults: 1, FragmentSelector: {FragmentSelectorType: PRODUCER_TIMESTAMP, TimestampRange: {StartTimestamp: T0, EndTimestamp: T0+30s}}} and keep the returned NextToken (its offset is 1, counted in fragments).
3. POST /getImages with {StreamName, ImageSelectorType: PRODUCER_TIMESTAMP, StartTimestamp: T0, EndTimestamp: T0+30s, Format: JPEG, NextToken: <the ListFragments token>} and NO SamplingInterval.
4. Observe HTTP 200 with "Images": [] and no NextToken, although the window demonstrably holds media (the same request without the token answers one image for T0).
5. Expected: the token names a different result set than the one the offset indexes into, so the request is refused with InvalidArgumentException (the same refusal the handler already sends when the token's stream or window disagree) rather than answered with an empty, terminal page.
