// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the COMPRESSION modules of the standard library a `python3.x`
//! function's run is given: `CreateFunction` then `Invoke` (`POST
//! /2015-03-31/functions/{FunctionName}/invocations`) over the router.
//!
//! The rule under test: a `python3.x` runtime presents the standard library of the runtime it
//! advertises, so `import bz2` and `import lzma` import cleanly and a buffer compressed by one
//! of them really is compressed — it decompresses back to exactly itself, whatever the data,
//! whichever level or container format was asked for and whichever of the module's interfaces
//! was used — while a stream the engine cannot read raises the error the module raises for one
//! rather than answering bytes.
//!
//! Every row drives real HTTP requests through the router, and the rows differ along what the
//! rule generalises over: WHICH payload is compressed (none, one byte, repeating text, every
//! byte value, and more bytes than the engine reads at a time), WHICH module and container
//! format carries it (bzip2, an `.xz` stream, the legacy `.lzma` stream, and that legacy
//! stream read by detection rather than by being named), WHICH level or preset was asked for,
//! and WHICH interface the handler drove (the whole-buffer calls, the incremental compressor
//! and decompressor fed in chunks, a decompressor read a piece at a time, the module's file
//! object and the module's `open`).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::invoke_request_id::{is_host_bound, one_invocation_stream_at_a_time};
use crate::python_frame_source_lines::{create_function, invoke, poll_until_active, router_on};

/// The bytes a bzip2 stream begins with, as hexadecimal. Source: the bzip2 format's own file
/// header, `BZh` followed by the block-size digit.
const BZIP2_HEAD: &str = "425a68";

/// The bytes an `.xz` stream begins with, as hexadecimal. Source: the xz format's own header
/// magic, `FD 37 7A 58 5A 00`.
const XZ_HEAD: &str = "fd377a585a00";

/// What one case of the probe answered, or `None` when the run was ended by a bound this whole
/// test binary shares rather than by anything the case asked for.
async fn answered(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    case: &str,
    event: &Value,
) -> Option<Value> {
    let answer = invoke(
        router,
        name,
        &serde_json::to_vec(event).expect("the event serializes"),
    )
    .await;
    if is_host_bound(answer.status, &answer.text()) {
        return None;
    }
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected the invocation to be answered 200, got {} {}",
        answer.status,
        answer.text()
    );
    let document = answer.json();
    assert_eq!(
        document.get("failed"),
        None,
        "case {case:?}: expected the handler to answer rather than fail, got {document}"
    );
    Some(document)
}

/// The value one member of an answer holds, named in the failure when it is missing.
fn member<'answer>(case: &str, document: &'answer Value, name: &str) -> &'answer Value {
    document.get(name).unwrap_or_else(|| {
        panic!("case {case:?}: expected the answer to carry {name:?}, got {document}")
    })
}

/// The text one member of an answer holds.
fn text(case: &str, document: &Value, name: &str) -> String {
    member(case, document, name)
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {name:?} to be text, got {document}")
        })
        .to_string()
}

/// The whole rule, driven END TO END as one test: one real `CreateFunction` and one real
/// `Invoke` per table over the router.
///
/// One test and one stream of invocations, the way the sibling runtime suites are built, for
/// the reason they are: the memory bound a run is stopped on is read from the whole process. A
/// table whose run was stopped by one of those shared bounds says nothing about the rule and is
/// left out of the reading — but not every one of them can be, so a test that read none of them
/// fails rather than passing silently.
#[tokio::test]
async fn the_python_runtime_compresses_and_reads_back_what_it_was_handed() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let probe = "probe-compression-streams";
    create_function(&router, probe, &[("index.py", PROBE_SOURCE)], "index.handler").await;
    poll_until_active(&router, probe).await;
    let mut read = 0;
    read += usize::from(every_stream_reads_back_as_the_bytes_it_was_made_of(&router, probe).await);
    read += usize::from(a_stream_the_engine_cannot_read_is_reported_as_one(&router, probe).await);
    assert!(
        read > 0,
        "expected at least one of the tables above to be answered by a run, got {read}: every \
         invocation reported a bound this whole test binary shares, so nothing of the rule was \
         read"
    );
}

/// The recorded shape and its generalisation: every payload compressed by either module reads
/// back as exactly the bytes it was made of.
///
/// Answers whether a run answered this table.
async fn every_stream_reads_back_as_the_bytes_it_was_made_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    probe: &str,
) -> bool {
    let case = "a payload compressed and read back";
    let Some(document) = answered(router, probe, case, &json!({"case": "round_trip"})).await else {
        return false;
    };
    let rows = member(case, &document, "rows")
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected the answer to carry rows, got {document}"))
        .clone();
    assert!(
        rows.len() >= 50,
        "case {case:?}: expected the probe to have driven a row per payload and interface, got \
         {} rows",
        rows.len()
    );
    for (row, read) in &rows {
        let handed = text(row, read, "payload");
        let restored = text(row, read, "restored");
        assert_eq!(
            restored, handed,
            "case {case:?} / {row}: expected the stream to read back as the bytes it was made \
             of, got {read}"
        );
        let compressed = member(row, read, "compressed").as_u64().unwrap_or_default();
        assert!(
            compressed > 0,
            "case {case:?} / {row}: expected a stream to have been written, got {read}"
        );
        // A payload with any repetition in it is SMALLER compressed, which is what says the
        // bytes really passed through a compressor rather than being carried through.
        if member(row, read, "compressible")
            .as_bool()
            .unwrap_or_default()
        {
            let handed_bytes = handed.len() / 2;
            assert!(
                (compressed as usize) < handed_bytes,
                "case {case:?} / {row}: expected {handed_bytes} bytes to compress to fewer, got \
                 {compressed}"
            );
        }
        // The stream carries the header of the format it was asked for.
        let head = text(row, read, "head");
        match text(row, read, "format").as_str() {
            "bz2" => assert!(
                head.starts_with(BZIP2_HEAD),
                "case {case:?} / {row}: expected a bzip2 stream to begin with {BZIP2_HEAD}, got \
                 {head}"
            ),
            "xz" => assert!(
                head.starts_with(XZ_HEAD),
                "case {case:?} / {row}: expected an .xz stream to begin with {XZ_HEAD}, got \
                 {head}"
            ),
            // The legacy format's header is the filter's own properties rather than a magic
            // number, so there is nothing to read off its first bytes.
            _ => assert!(
                !head.is_empty(),
                "case {case:?} / {row}: expected a stream to have been written, got {read}"
            ),
        }
    }
    true
}

/// RULE: a stream the engine cannot read is reported as the failure the module raises for one,
/// and the bytes a stream was followed by are the ones the module reads the next stream out of.
///
/// Answers whether a run answered this table.
async fn a_stream_the_engine_cannot_read_is_reported_as_one(
    router: &smithy_server_runtime::MultiServiceRouter,
    probe: &str,
) -> bool {
    let case = "a stream nothing can be read out of";
    let Some(document) = answered(router, probe, case, &json!({"case": "refusals"})).await else {
        return false;
    };
    // (the row, the member, what the module answers)
    let rows: &[(&str, &str, &str)] = &[
        (
            "bytes that are no bzip2 stream at all",
            "bz2_garbage",
            "OSError",
        ),
        (
            "bytes that are no lzma stream at all",
            "lzma_garbage",
            "LZMAError",
        ),
        (
            "a bzip2 stream that ends before its end-of-stream marker",
            "bz2_truncated_file",
            "EOFError",
        ),
        (
            "an .xz stream that ends before its end-of-stream marker",
            "lzma_truncated_file",
            "EOFError",
        ),
        (
            "a compression level the format does not have",
            "bz2_level",
            "ValueError",
        ),
        (
            "an integrity check the format does not have",
            "lzma_check",
            "LZMAError",
        ),
    ];
    for (row, name, raised) in rows {
        assert_eq!(
            text(row, &document, name),
            *raised,
            "case {case:?} / {row}: expected the module's own error class, got {document}"
        );
    }
    // No bytes at all decompress to no bytes at all, which is what the whole-buffer call
    // answers for an empty buffer rather than raising.
    assert_eq!(
        text(case, &document, "bz2_nothing"),
        "",
        "case {case:?}: expected an empty buffer to decompress to nothing, got {document}"
    );
    // A stream that has not all arrived is one the decompressor is waiting for the rest of,
    // rather than a failure.
    assert_eq!(
        member(case, &document, "bz2_partial_needs_input"),
        &json!(true),
        "case {case:?}: expected a truncated stream to be waiting for its rest, got {document}"
    );
    assert_eq!(
        member(case, &document, "bz2_partial_eof"),
        &json!(false),
        "case {case:?}: expected a truncated stream not to have ended, got {document}"
    );
    // What followed a stream is what the next stream is read out of: the second of two
    // concatenated streams, and the bytes of something else.
    assert_eq!(
        text(case, &document, "bz2_concatenated"),
        "firstsecond",
        "case {case:?}: expected both streams of a concatenated file, got {document}"
    );
    assert_eq!(
        text(case, &document, "bz2_unused_data"),
        "tail",
        "case {case:?}: expected what followed the stream to be left over, got {document}"
    );
    true
}

/// The probe every row drives: its handler answers what the case the event names came to.
const PROBE_SOURCE: &str = r#"
import bz2
import io
import lzma

# The PAYLOADS every interface below carries, and whether each has enough repetition in it to
# come out smaller than it went in.
PAYLOADS = (
    ("no bytes at all", b"", False),
    ("a single byte", b"\x00", False),
    ("repeating text", b"lambda compression " * 512, True),
    ("every byte value", bytes(range(256)) * 4, True),
    ("more bytes than one read of the engine", bytes(range(256)) * 300, True),
)

# How many bytes of a payload are handed to an incremental compressor at a time, and how many
# bytes of a stream to an incremental decompressor: neither divides any payload or stream
# evenly, so a row reads a stream whose pieces fall where they fall.
_WRITE_CHUNK = 1000
_READ_CHUNK = 97

# How many bytes a decompressor read a piece at a time is asked for.
_PIECE = 11


def _written_in_chunks(compressor, data):
    """`data` handed to `compressor` a chunk at a time, then the stream it flushed."""
    parts = []
    for at in range(0, len(data), _WRITE_CHUNK):
        parts.append(compressor.compress(data[at:at + _WRITE_CHUNK]))
    parts.append(compressor.flush())
    return b"".join(parts)


def _read_in_chunks(decompressor, stream):
    """The stream handed to `decompressor` a chunk at a time."""
    parts = []
    at = 0
    while at < len(stream) and not decompressor.eof:
        parts.append(decompressor.decompress(stream[at:at + _READ_CHUNK]))
        at += _READ_CHUNK
    return b"".join(parts)


def _read_in_pieces(decompressor, stream):
    """The stream read back a few bytes at a time, which is what a maximum length asks for."""
    parts = [decompressor.decompress(stream, _PIECE)]
    while not decompressor.eof:
        parts.append(decompressor.decompress(b"", _PIECE))
    return b"".join(parts)


def _through_file(opening, data):
    """`data` written through one of the module's file objects, and the bytes it wrote."""
    held = io.BytesIO()
    with opening(held, "wb") as handle:
        handle.write(data)
    return held.getvalue()


def _out_of_file(opening, stream):
    """The bytes read back out of one of the module's file objects."""
    with opening(io.BytesIO(stream), "rb") as handle:
        return handle.read()


CODECS = (
    ("bz2 whole buffer", "bz2", bz2.compress, bz2.decompress),
    ("bz2 whole buffer at the lowest level", "bz2", lambda data: bz2.compress(data, 1), bz2.decompress),
    ("bz2 whole buffer at the highest level", "bz2", lambda data: bz2.compress(data, 9), bz2.decompress),
    (
        "bz2 handed over in chunks",
        "bz2",
        lambda data: _written_in_chunks(bz2.BZ2Compressor(), data),
        lambda stream: _read_in_chunks(bz2.BZ2Decompressor(), stream),
    ),
    (
        "bz2 read a piece at a time",
        "bz2",
        bz2.compress,
        lambda stream: _read_in_pieces(bz2.BZ2Decompressor(), stream),
    ),
    (
        "bz2 through the module's file object",
        "bz2",
        lambda data: _through_file(bz2.BZ2File, data),
        lambda stream: _out_of_file(bz2.BZ2File, stream),
    ),
    (
        "bz2 through the module's open",
        "bz2",
        lambda data: _through_file(bz2.open, data),
        lambda stream: _out_of_file(bz2.open, stream),
    ),
    ("lzma an .xz stream", "xz", lzma.compress, lzma.decompress),
    (
        "lzma an .xz stream at the lowest preset",
        "xz",
        lambda data: lzma.compress(data, preset=1),
        lzma.decompress,
    ),
    (
        "lzma the legacy .lzma format",
        "alone",
        lambda data: lzma.compress(data, format=lzma.FORMAT_ALONE),
        lambda stream: lzma.decompress(stream, format=lzma.FORMAT_ALONE),
    ),
    (
        "lzma the legacy format read by detection",
        "alone",
        lambda data: lzma.compress(data, format=lzma.FORMAT_ALONE),
        lzma.decompress,
    ),
    (
        "lzma handed over in chunks",
        "xz",
        lambda data: _written_in_chunks(lzma.LZMACompressor(), data),
        lambda stream: _read_in_chunks(lzma.LZMADecompressor(), stream),
    ),
    (
        "lzma read a piece at a time",
        "xz",
        lzma.compress,
        lambda stream: _read_in_pieces(lzma.LZMADecompressor(), stream),
    ),
    (
        "lzma through the module's file object",
        "xz",
        lambda data: _through_file(lzma.LZMAFile, data),
        lambda stream: _out_of_file(lzma.LZMAFile, stream),
    ),
    (
        "lzma through the module's open",
        "xz",
        lambda data: _through_file(lzma.open, data),
        lambda stream: _out_of_file(lzma.open, stream),
    ),
)


def _round_trip(event):
    """Every payload through every interface, and what came back."""
    rows = {}
    for name, payload, compressible in PAYLOADS:
        for label, format, squeeze, expand in CODECS:
            squeezed = squeeze(payload)
            rows[name + " / " + label] = {
                "payload": payload.hex(),
                "restored": expand(squeezed).hex(),
                "compressed": len(squeezed),
                "compressible": compressible,
                "format": format,
                "head": squeezed[:8].hex(),
            }
    return {"rows": rows}


def _raised(call):
    """The class of the error `call` raised, or what it answered instead."""
    try:
        call()
    except BaseException as error:
        return type(error).__name__
    return "no error"


def _refusals(event):
    """What a stream nothing can be read out of is reported as."""
    answered = {}
    answered["bz2_garbage"] = _raised(lambda: bz2.decompress(b"not a stream at all"))
    answered["lzma_garbage"] = _raised(lambda: lzma.decompress(b"not a stream at all"))
    truncated = bz2.compress(b"lambda compression " * 64)[:-10]
    answered["bz2_truncated_file"] = _raised(lambda: _out_of_file(bz2.BZ2File, truncated))
    cut = lzma.compress(b"lambda compression " * 64)[:-10]
    answered["lzma_truncated_file"] = _raised(lambda: _out_of_file(lzma.LZMAFile, cut))
    answered["bz2_level"] = _raised(lambda: bz2.BZ2Compressor(0))
    answered["lzma_check"] = _raised(lambda: lzma.LZMACompressor(check=7))
    answered["bz2_nothing"] = bz2.decompress(b"").hex()
    waiting = bz2.BZ2Decompressor()
    waiting.decompress(truncated)
    answered["bz2_partial_needs_input"] = waiting.needs_input
    answered["bz2_partial_eof"] = waiting.eof
    answered["bz2_concatenated"] = bz2.decompress(
        bz2.compress(b"first") + bz2.compress(b"second")
    ).decode("ascii")
    leftover = bz2.BZ2Decompressor()
    leftover.decompress(bz2.compress(b"first") + b"tail")
    answered["bz2_unused_data"] = leftover.unused_data.decode("ascii")
    return answered


_CASES = {"round_trip": _round_trip, "refusals": _refusals}


def handler(event, context):
    try:
        return _CASES[event["case"]](event)
    except BaseException as error:
        return {"failed": type(error).__name__ + ": " + str(error)}
"#;
