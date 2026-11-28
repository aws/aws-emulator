// SPDX-License-Identifier: CC0-1.0

//! The COMPRESSION half of this runtime's standard library: the `bz2` and `lzma` modules a
//! Python handler imports, and the libraries behind them.
//!
//! A `python3.x` runtime is a promise about an interpreter AND about the standard library that
//! comes with it: a handler whose module body says `import bz2` or `import lzma` imports
//! cleanly on the runtime it was written for, and the bytes it then compresses really are
//! compressed. Both of those modules are written in Python over an EXTENSION module of their
//! own (`_bz2`, `_lzma`), and an interpreter that registers no such extension cannot import
//! either — which is what made a handler importing one fail here. So this module supplies the
//! two extensions, exactly as the socket interface, the TLS layer, the database interface and
//! the time-zone interface are supplied ([`super::guest_http::runtime_stdlib_modules`],
//! [`super::guest_tls`], [`super::guest_sqlite`], [`super::guest_zoneinfo`], which this
//! module's sources are resolved beside), so the library's OWN `bz2.py` and `lzma.py` — the
//! whole-buffer calls, the incremental compressor and decompressor, the file objects and
//! `open` — are the ones a handler drives.
//!
//! What is this module's own:
//!
//! * the two EXTENSION modules ([`BZ2_SOURCE`], [`LZMA_SOURCE`]): the compressor and
//!   decompressor objects the library builds on, with the state each one carries (what it has
//!   been handed, whether the stream has ended, what was left over after it) and the module
//!   constants and error classes the library re-exports;
//! * the ENGINES the bytes really pass through, both libraries computing over the bytes they
//!   are handed in this process: `bzip2` on its Rust backend for a bzip2 stream, and the xz
//!   project's own `liblzma`, built from the sources that crate vendors and linked into this
//!   binary, for an `.xz` or a legacy `.lzma` one. No host path is opened, no shared library is
//!   loaded and nothing leaves this process;
//! * the BOUNDS of one call, held here where no guest code can reach them: how many bytes one
//!   call may be handed and how many it may answer, and how much memory the engine may use to
//!   read one stream — so a handler cannot turn a compression call into an unbounded
//!   allocation.
//!
//! One consequence of an engine that works on a whole buffer is stated rather than hidden: a
//! stream handed over a chunk at a time is read from its beginning each time, so reading a
//! large stream in small pieces costs more than reading it in one. An `.xz` stream that is not
//! all here yet is answered from its own framing without being read at all ([`framed`]), and
//! every call is bounded by [`MAX_CALL_BYTES`] and by what the invocation has left.
//!
//! Nothing here invents an answer: a compressed stream is the one the engine produced, a
//! decompressed buffer is what the engine read out of the bytes it was given, and a stream the
//! engine could not read is reported as the failure the library raises for one — never as
//! bytes.

use ::std::io::{Cursor, Read, Write};

use ::serde_json::{json, Value};

use super::package::PackageModule;

/// The modules of the standard library this half of the runtime supplies: the two extension
/// modules `bz2` and `lzma` are written over.
///
/// Resolved before the frozen standard library and after the deployment package's own entries,
/// so the library's own `bz2.py` and `lzma.py` import THESE and a package shipping its own
/// still wins.
#[must_use]
pub fn runtime_stdlib_modules() -> Vec<PackageModule> {
    [
        ("_lambda_compression.py", ENGINE_SOURCE),
        ("_bz2.py", BZ2_SOURCE),
        ("_lzma.py", LZMA_SOURCE),
    ]
    .into_iter()
    .map(|(path, source)| PackageModule {
        path: path.to_string(),
        source: source.to_string(),
    })
    .collect()
}

/// The most bytes ONE call may be handed, and the most it may answer.
///
/// CHOSEN, like every other bound this crate holds a guest-to-host seam to: a handler decides
/// how much it compresses, and both the buffer handed over and the one answered are held in
/// this process's memory. It is far above the payloads and files a function compresses in one
/// invocation, and low enough that one call cannot fill this process.
const MAX_CALL_BYTES: usize = 64 * 1024 * 1024;

/// How many bytes a decompression reads out at a time before it checks what it has answered
/// against the bound above.
const READ_CHUNK: usize = 64 * 1024;

/// What ONE call of the compression seam answers, as the JSON document the guest reads.
///
/// `verb` names the engine and the direction (`bz2.compress`, `lzma.decompress`), and
/// `document` carries the bytes — hexadecimal, which is how this runtime's guest-to-host seams
/// carry bytes — with whatever that call is parameterised by. A failure is answered as a
/// failure (`error`), which is what the library raises for a stream it could not read, rather
/// than as bytes nothing produced.
#[must_use]
pub fn answered(verb: &str, document: &str) -> String {
    let asked: Value = match ::serde_json::from_str(document) {
        Ok(asked) => asked,
        Err(refused) => return refusal(&format!("the call could not be read: {refused}")),
    };
    let data = match bytes_of(&asked) {
        Ok(data) => data,
        Err(refused) => return refusal(&refused),
    };
    match verb {
        "bz2.compress" => compressed(bz2_compressed(&data, level_of(&asked))),
        "bz2.decompress" => decompressed(bz2_decompressed(&data)),
        "lzma.compress" => compressed(lzma_compressed(
            &data,
            format_of(&asked),
            preset_of(&asked),
            check_asked(&asked),
        )),
        "lzma.decompress" => decompressed(lzma_decompressed(&data, format_of(&asked))),
        other => refusal(&format!("{other} is no call of this compression interface")),
    }
}

/// The bytes one call was handed, read out of the hexadecimal the seam carries them as and
/// held inside [`MAX_CALL_BYTES`].
fn bytes_of(asked: &Value) -> Result<Vec<u8>, String> {
    let held = asked.get("data").and_then(Value::as_str).unwrap_or_default();
    if held.len() / 2 > MAX_CALL_BYTES {
        return Err(format!(
            "one compression call may be handed at most {MAX_CALL_BYTES} bytes"
        ));
    }
    ::hex::decode(held).map_err(|refused| format!("the bytes could not be read: {refused}"))
}

/// The compression LEVEL one call names, held inside the range a bzip2 stream has
/// (`1` to `9`), and the library's own default when it names none.
fn level_of(asked: &Value) -> u32 {
    let named = asked.get("level").and_then(Value::as_u64).unwrap_or(9);
    u32::try_from(named).unwrap_or(9).clamp(1, 9)
}

/// The container FORMAT one call names: an `.xz` stream, the legacy `.lzma` stream of a bare
/// LZMA1 header, or — for a decompression — whichever of the two the bytes themselves are.
fn format_of(asked: &Value) -> &'static str {
    match asked.get("format").and_then(Value::as_str).unwrap_or("xz") {
        "alone" => "alone",
        "auto" => "auto",
        _ => "xz",
    }
}

/// The PRESET one call names, held inside the range the format has (`0` to `9`, with the
/// extreme flag the format sets above them kept), and the format's own default when it names
/// none.
fn preset_of(asked: &Value) -> u32 {
    let Some(named) = asked.get("preset").and_then(Value::as_u64) else {
        return DEFAULT_PRESET;
    };
    let named = u32::try_from(named).unwrap_or(DEFAULT_PRESET);
    let extreme = named & EXTREME_PRESET;
    (named & !EXTREME_PRESET).min(9) | extreme
}

/// The preset an `.xz` stream is written at when the call names none. Source: the default
/// preset of the format itself, which is what `lzma.PRESET_DEFAULT` names.
const DEFAULT_PRESET: u32 = 6;

/// The flag the format sets on a preset to ask for its extreme form, which is what
/// `lzma.PRESET_EXTREME` names.
const EXTREME_PRESET: u32 = 1 << 31;

/// The INTEGRITY CHECK one call names, by the identifier the format gives it, and `None` when
/// it names none — which is the format's own default.
fn check_asked(asked: &Value) -> Option<u32> {
    let named = asked.get("check").and_then(Value::as_i64)?;
    u32::try_from(named).ok()
}

/// One compressed stream, as the answer the guest reads.
fn compressed(outcome: Result<Vec<u8>, String>) -> String {
    match outcome {
        Ok(out) => json!({ "out": ::hex::encode(out) }).to_string(),
        Err(refused) => refusal(&refused),
    }
}

/// What one decompression read out: the bytes, how many of the buffer it consumed, and whether
/// the stream it read ENDED — which is what the library reports as `eof` and what decides
/// whether anything was left over after the stream.
fn decompressed(outcome: Result<(Vec<u8>, usize, bool), String>) -> String {
    match outcome {
        Ok((out, consumed, eof)) => json!({
            "out": ::hex::encode(out),
            "consumed": consumed,
            "eof": eof,
        })
        .to_string(),
        Err(refused) => refusal(&refused),
    }
}

/// A call this seam could not serve, as the answer the guest reads: the library raises its own
/// error for it rather than reading bytes out of the answer.
fn refusal(detail: &str) -> String {
    json!({ "error": detail }).to_string()
}

/// COMPRESS `data` as one bzip2 stream, at `level`.
fn bz2_compressed(data: &[u8], level: u32) -> Result<Vec<u8>, String> {
    let mut encoder = ::bzip2::write::BzEncoder::new(
        Vec::with_capacity(data.len() / 2 + 64),
        ::bzip2::Compression::new(level),
    );
    encoder
        .write_all(data)
        .map_err(|refused| format!("the bzip2 stream could not be written: {refused}"))?;
    encoder
        .finish()
        .map_err(|refused| format!("the bzip2 stream could not be written: {refused}"))
}

/// DECOMPRESS the bzip2 stream `data` begins with, answering how many bytes of the buffer that
/// stream took — a bzip2 file being one or more streams concatenated, the library reads the
/// next one out of what was left over.
///
/// A buffer that ends BEFORE the stream does is no failure: it is a stream whose rest has not
/// been handed over yet, which is what the library's incremental decompressor reports as
/// needing more input.
fn bz2_decompressed(data: &[u8]) -> Result<(Vec<u8>, usize, bool), String> {
    let mut decoder = ::bzip2::read::BzDecoder::new(Cursor::new(data));
    match read_out(&mut decoder)? {
        Some(out) => {
            let consumed = usize::try_from(decoder.total_in())
                .unwrap_or(data.len())
                .min(data.len());
            Ok((out, consumed, true))
        }
        None => Ok((Vec::new(), 0, false)),
    }
}

/// COMPRESS `data` as one `.xz` stream, or as the legacy `.lzma` stream the other format is,
/// at `preset` and — for an `.xz` stream, which is the format that carries one — with the
/// integrity check asked for.
fn lzma_compressed(
    data: &[u8],
    format: &str,
    preset: u32,
    check: Option<u32>,
) -> Result<Vec<u8>, String> {
    let stream = if format == "alone" {
        let options = ::liblzma::stream::LzmaOptions::new_preset(preset)
            .map_err(|refused| format!("the lzma stream could not be written: {refused}"))?;
        ::liblzma::stream::Stream::new_lzma_encoder(&options)
    } else {
        ::liblzma::stream::Stream::new_easy_encoder(preset, check_of(check))
    }
    .map_err(|refused| format!("the lzma stream could not be written: {refused}"))?;
    let mut encoder = ::liblzma::write::XzEncoder::new_stream(
        Vec::with_capacity(data.len() / 2 + 64),
        stream,
    );
    encoder
        .write_all(data)
        .map_err(|refused| format!("the lzma stream could not be written: {refused}"))?;
    encoder
        .finish()
        .map_err(|refused| format!("the lzma stream could not be written: {refused}"))
}

/// The INTEGRITY CHECK an `.xz` stream is written with: the one the call named, by the
/// identifier the format gives it, and the format's own default when it named none.
fn check_of(check: Option<u32>) -> ::liblzma::stream::Check {
    match check {
        Some(0) => ::liblzma::stream::Check::None,
        Some(1) => ::liblzma::stream::Check::Crc32,
        Some(10) => ::liblzma::stream::Check::Sha256,
        _ => ::liblzma::stream::Check::Crc64,
    }
}

/// DECOMPRESS the `.xz` or `.lzma` stream `data` holds, in the format named or in whichever of
/// the two the bytes themselves are.
///
/// As with a bzip2 stream, a buffer that ends before the stream does is reported as needing
/// more input rather than as a failure, and how many bytes the stream took is answered so the
/// library can read what followed it.
fn lzma_decompressed(data: &[u8], format: &str) -> Result<(Vec<u8>, usize, bool), String> {
    // A stream whose own framing says it is not all here yet is one whose rest has not been
    // handed over: it is read when it is complete, rather than reported as a stream that could
    // not be read.
    if !framed(data, format) {
        return Ok((Vec::new(), 0, false));
    }
    let stream = match format {
        "alone" => ::liblzma::stream::Stream::new_lzma_decoder(MEMORY_LIMIT),
        "auto" => ::liblzma::stream::Stream::new_auto_decoder(MEMORY_LIMIT, 0),
        _ => ::liblzma::stream::Stream::new_stream_decoder(MEMORY_LIMIT, 0),
    }
    .map_err(|refused| format!("the lzma stream could not be read: {refused}"))?;
    let mut decoder = ::liblzma::read::XzDecoder::new_stream(Cursor::new(data), stream);
    match read_out(&mut decoder)? {
        Some(out) => {
            let consumed = usize::try_from(decoder.total_in())
                .unwrap_or(data.len())
                .min(data.len());
            Ok((out, consumed, true))
        }
        None => Ok((Vec::new(), 0, false)),
    }
}

/// How much memory the engine may use to read ONE stream.
///
/// CHOSEN, for the reason [`MAX_CALL_BYTES`] is: the dictionary a stream names decides how much
/// the engine allocates to read it, and a handler decides which stream it hands over. It is far
/// above the dictionary of the highest preset the format has, and a stream naming more than
/// this is refused by the engine rather than allocated for.
const MEMORY_LIMIT: u64 = 256 * 1024 * 1024;

/// Whether `data` carries a WHOLE stream of `format`, as that format's own framing says.
///
/// An `.xz` stream ends with its own stream footer, whose last two bytes are the footer magic
/// (`YZ`), so a buffer not ending in one is a stream whose rest has not arrived. The legacy
/// `.lzma` stream has no end of its own to read — it is a header and then the coded bytes — so
/// all that can be said of one is whether its header is there at all. A buffer whose format was
/// not named is judged as the format its own first bytes say it is.
fn framed(data: &[u8], format: &str) -> bool {
    let format = if format == "auto" {
        if data.len() < XZ_MAGIC.len() {
            return false;
        }
        if data.starts_with(XZ_MAGIC) {
            "xz"
        } else {
            "alone"
        }
    } else {
        format
    };
    if format == "alone" {
        return data.len() >= ALONE_HEADER_BYTES;
    }
    data.len() > XZ_FOOTER_BYTES && data.ends_with(XZ_FOOTER_MAGIC)
}

/// How many bytes the header of a legacy `.lzma` stream takes: the filter's own properties,
/// the dictionary size and the uncompressed size.
const ALONE_HEADER_BYTES: usize = 13;

/// How many bytes the stream footer of an `.xz` stream takes.
const XZ_FOOTER_BYTES: usize = 12;

/// The bytes that footer ENDS with, which is what says the stream is all here.
const XZ_FOOTER_MAGIC: &[u8] = b"YZ";

/// The bytes an `.xz` stream begins with, which is what says a buffer whose format was not
/// named is one.
const XZ_MAGIC: &[u8] = &[0xFD, b'7', b'z', b'X', b'Z', 0x00];

/// Read everything `stream` decompresses to, inside [`MAX_CALL_BYTES`], and answer `None` when
/// the stream ENDED BEFORE it was complete — a stream whose rest has not been handed over.
fn read_out<R: Read>(mut stream: R) -> Result<Option<Vec<u8>>, String> {
    let mut out = Vec::new();
    let mut chunk = vec![0_u8; READ_CHUNK];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(Some(out)),
            Ok(read) => {
                if out.len() + read > MAX_CALL_BYTES {
                    return Err(format!(
                        "one compression call may answer at most {MAX_CALL_BYTES} bytes"
                    ));
                }
                out.extend_from_slice(&chunk[..read]);
            }
            Err(refused) if refused.kind() == ::std::io::ErrorKind::UnexpectedEof => {
                return Ok(None)
            }
            Err(refused) => return Err(format!("the stream could not be read: {refused}")),
        }
    }
}

/// `_lambda_compression`: the seam both extension modules call, and the incremental stream
/// objects they are built out of.
///
/// One module rather than the same machinery in each: a compressor and a decompressor over an
/// engine that works on a whole buffer differ only in which engine they name and which error
/// class they raise.
const ENGINE_SOURCE: &str = r#"
"""The compression engines of this runtime, and the streams the extensions build on them."""

import json as _json

_host = __import__("_lambda_host")


def performed(verb, asked):
    """ONE call of the compression seam: the document the host answered."""
    return _json.loads(_host.compression(verb, _json.dumps(asked)))


class Compressor:
    """An incremental compressor over an engine that compresses a whole buffer.

    What is handed in is HELD until the stream is flushed, and the whole stream is answered
    there — which is what a `compress` call answering no bytes yet means, and what the
    interface allows of every one of them.
    """

    # The engine the subclass names, and the error class it raises for a stream the engine
    # could not write.
    engine = ""
    error = OSError

    def __init__(self, **asked):
        self._asked = asked
        self._held = bytearray()
        self._flushed = False

    def compress(self, data):
        """Hold `data` for the stream this compressor is building."""
        if self._flushed:
            raise ValueError("Compressor has been flushed")
        self._held += bytes(data)
        return b""

    def flush(self):
        """End the stream and answer it."""
        if self._flushed:
            raise ValueError("Repeated call to flush()")
        self._flushed = True
        asked = dict(self._asked)
        asked["data"] = bytes(self._held).hex()
        answered = performed(self.engine + ".compress", asked)
        if "error" in answered:
            raise self.error(answered["error"])
        return bytes.fromhex(answered["out"])


class Decompressor:
    """An incremental decompressor over that same kind of engine.

    What is handed in is held until the stream it belongs to is complete, which is what
    `needs_input` reports; the bytes are answered as the caller asks for them, the end of the
    stream is `eof` and what followed the stream is `unused_data`.
    """

    engine = ""
    error = OSError
    # What a call made after the end of the stream raises, as the extension words it.
    ended = "End of stream already reached"

    def __init__(self, **asked):
        self._asked = asked
        self._held = bytearray()
        self._read = b""
        self._answered = 0
        self._ended = False
        self._unused = b""
        self.eof = False
        self.needs_input = True
        self.unused_data = b""

    def decompress(self, data, max_length=-1):
        """Read as much of this stream as the bytes handed over so far hold.

        At most `max_length` bytes are answered when it is not negative; the rest is answered
        by the calls after it, which is what `needs_input` being false says is waiting.
        """
        if self.eof:
            raise EOFError(self.ended)
        self._held += bytes(data)
        if not self._ended:
            self._read_stream()
        answered = self._read[self._answered:]
        if max_length is not None and max_length >= 0:
            answered = answered[:max_length]
        self._answered += len(answered)
        standing = self._answered < len(self._read)
        # The stream is over for the CALLER once its last byte has been answered, which is
        # what `eof` says: a reader that has been given everything asks for no more.
        self.eof = self._ended and not standing
        self.needs_input = not self._ended and not standing
        if self.eof:
            self.unused_data = self._unused
        return bytes(answered)

    def _read_stream(self):
        """Read the stream the held bytes carry, as far as they carry it."""
        asked = dict(self._asked)
        asked["data"] = bytes(self._held).hex()
        answered = performed(self.engine + ".decompress", asked)
        if "error" in answered:
            raise self.error(answered["error"])
        self._read = bytes.fromhex(answered["out"])
        if answered["eof"]:
            self._ended = True
            self._unused = bytes(self._held[answered["consumed"]:])
"#;

/// `_bz2`: the extension module the standard library's own `bz2` is written over.
const BZ2_SOURCE: &str = r#"
"""The bzip2 engine of this runtime, as the module `bz2` is written over."""

import _lambda_compression as _engine


class BZ2Compressor(_engine.Compressor):
    """An incremental compressor answering one bzip2 stream."""

    engine = "bz2"

    def __init__(self, compresslevel=9):
        if not 1 <= compresslevel <= 9:
            raise ValueError("compresslevel must be between 1 and 9")
        _engine.Compressor.__init__(self, level=compresslevel)


class BZ2Decompressor(_engine.Decompressor):
    """An incremental decompressor reading one bzip2 stream.

    A bzip2 file is one or more streams concatenated, and what followed the stream this
    decompressor read is `unused_data` — which is where the library reads the next one from.
    """

    engine = "bz2"
"#;

/// `_lzma`: the extension module the standard library's own `lzma` is written over.
const LZMA_SOURCE: &str = r#"
"""The lzma engine of this runtime, as the module `lzma` is written over."""

import _lambda_compression as _engine

# The container FORMATS this interface names, by the numbers the library names them with.
FORMAT_AUTO = 0
FORMAT_XZ = 1
FORMAT_ALONE = 2
FORMAT_RAW = 3

# The INTEGRITY CHECKS an `.xz` stream can carry, by their own identifiers.
CHECK_NONE = 0
CHECK_CRC32 = 1
CHECK_CRC64 = 4
CHECK_SHA256 = 10
CHECK_ID_MAX = 15
CHECK_UNKNOWN = 16

# The FILTERS a filter chain is written out of, by their own identifiers.
FILTER_LZMA1 = 0x4000000000000001
FILTER_LZMA2 = 0x21
FILTER_DELTA = 0x03
FILTER_X86 = 0x04
FILTER_POWERPC = 0x05
FILTER_IA64 = 0x06
FILTER_ARM = 0x07
FILTER_ARMTHUMB = 0x08
FILTER_SPARC = 0x09

# The MATCH FINDERS and the COMPRESSION MODES a filter can name.
MF_HC3 = 0x03
MF_HC4 = 0x04
MF_BT2 = 0x12
MF_BT3 = 0x13
MF_BT4 = 0x14
MODE_FAST = 1
MODE_NORMAL = 2

# The PRESETS a compression level is named by.
PRESET_DEFAULT = 6
PRESET_EXTREME = 1 << 31

# What a RAW stream, and a chain of filters written out by the caller, are answered with: the
# engine of this runtime reads and writes the two container formats, and a raw stream is a
# filter chain with no container to say what the filters were.
_NO_FILTER_CHAIN = (
    "this runtime's lzma reads and writes the .xz and .lzma formats, not a raw filter chain"
)


class LZMAError(Exception):
    """Raised when the engine could not read or write a stream."""


def is_check_supported(check_id):
    """Whether a stream carrying the integrity check of this identifier can be read."""
    return check_id in (CHECK_NONE, CHECK_CRC32, CHECK_CRC64, CHECK_SHA256)


def _encode_filter_properties(filterspec):
    """The properties of one filter, as a raw stream carries them."""
    raise LZMAError(_NO_FILTER_CHAIN)


def _decode_filter_properties(filter_id, encoded_props):
    """The filter one raw stream's properties describe."""
    raise LZMAError(_NO_FILTER_CHAIN)


def _container(format, filters, reading):
    """The container the engine is asked for, refusing what it has no reading of."""
    if filters is not None:
        raise LZMAError(_NO_FILTER_CHAIN)
    if format == FORMAT_XZ:
        return "xz"
    if format == FORMAT_ALONE:
        return "alone"
    if format == FORMAT_AUTO and reading:
        return "auto"
    if format == FORMAT_RAW:
        raise LZMAError(_NO_FILTER_CHAIN)
    raise LZMAError("Invalid container format: %s" % (format,))


class LZMACompressor(_engine.Compressor):
    """An incremental compressor answering one `.xz` or `.lzma` stream."""

    engine = "lzma"
    error = LZMAError

    def __init__(self, format=FORMAT_XZ, check=-1, preset=None, filters=None):
        if check != -1 and not is_check_supported(check):
            raise LZMAError("Invalid integrity check: %s" % (check,))
        asked = {"format": _container(format, filters, False)}
        if preset is not None:
            asked["preset"] = preset
        if check != -1:
            asked["check"] = check
        _engine.Compressor.__init__(self, **asked)


class LZMADecompressor(_engine.Decompressor):
    """An incremental decompressor reading one `.xz` or `.lzma` stream."""

    engine = "lzma"
    error = LZMAError
    ended = "Already at end of stream"

    def __init__(self, format=FORMAT_AUTO, memlimit=None, filters=None):
        _engine.Decompressor.__init__(self, format=_container(format, filters, True))
        # Which integrity check the stream carried: the engine verifies the check an `.xz`
        # stream states for itself and reports none of its own, so this stays what the library
        # names for a check that is not known.
        self.check = CHECK_UNKNOWN
"#;
