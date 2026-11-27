Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# The page-token codec is implemented three times across this crate's listings, so the three copies can drift

_Filed: 2026-11-27T23:24:48Z_
_Operation: ListStreams | Class: maintainability_

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The same page-token rule — base64-encode a marker prefix followed by the resume key, and on the way back decode base64, require the prefix, and yield the key — is written out three times in this crate, once per paginated listing. The three copies are byte-for-byte the same logic, differing only in the prefix constant and the name of the key they carry. Nothing shares them, so a fix or a change to one (for example to how a non-decodable token is distinguished from one minted by another listing) silently leaves the other two behind.

The deliberate part of the design — a DISTINCT prefix per listing, so a token minted by one listing is not a resume point for another — is preserved by a single parameterised helper taking the prefix; it does not require three copies of the codec. Behaviour and every existing test stay unchanged: the helper mints and accepts exactly the same strings.

## Evidence

Copy 1 — `src/ops/list_streams.rs:247-263`:
```
const PAGE_TOKEN_PREFIX: &str = "kinesisvideo/streams/v1/";
...
fn encode_token(stream_name: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{PAGE_TOKEN_PREFIX}{stream_name}"))
}
...
fn decode_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    decoded.strip_prefix(PAGE_TOKEN_PREFIX).map(str::to_string)
}
```
Copy 2 — `src/ops/list_signaling_channels.rs:262-277`:
```
const PAGE_TOKEN_PREFIX: &str = "kinesisvideo/signalingchannels/v1/";
...
fn encode_token(channel_name: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{PAGE_TOKEN_PREFIX}{channel_name}"))
}
...
fn decode_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    decoded.strip_prefix(PAGE_TOKEN_PREFIX).map(str::to_string)
}
```
Copy 3 — `src/ops/describe_mapped_resource_configuration.rs:243-260`:
```
const PAGE_TOKEN_PREFIX: &str = "kinesisvideo/mappedresources/v1/";
...
fn encode_token(arn: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{PAGE_TOKEN_PREFIX}{arn}"))
}
...
fn decode_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    decoded.strip_prefix(PAGE_TOKEN_PREFIX).map(str::to_string)
}
```
The copies even admit they are each other's restatement — `src/ops/describe_mapped_resource_configuration.rs:250`: "The codec is `aws_smithy_types::base64`, already in this crate's dependency tree and what the sibling listings mint their tokens with."

Tests that pin the behaviour and must stay green unchanged: `tests/all/stream_lifecycle.rs:643` `list_streams_honours_max_results_as_a_page_bound`, `tests/all/stream_lifecycle.rs:877` `list_streams_states_its_verdict_on_a_next_token`, `tests/all/signaling_channel_lifecycle.rs:977` `the_listing_pages_and_filters_over_the_stored_index` (and its rejected token at `tests/all/signaling_channel_lifecycle.rs:572`), `tests/all/stream_configuration.rs:735` `a_mapped_resource_is_reported_from_the_mapping_that_created_it` (its rejected token at `tests/all/stream_configuration.rs:856`).

## Steps to Reproduce

1. Open src/ops/list_streams.rs:247-263, src/ops/list_signaling_channels.rs:262-277 and src/ops/describe_mapped_resource_configuration.rs:243-260 side by side
2. Observe the three identical encode_token/decode_token bodies, differing only in the PAGE_TOKEN_PREFIX constant and the parameter name
3. Change the decode rule in one file (for example, to reject a token whose prefix belongs to another listing with a distinct message) and observe nothing forces the other two copies to follow
