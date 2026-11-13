// SPDX-License-Identifier: CC0-1.0

//! `buffer`: the byte container a Node program reads and writes binary with, and the
//! `Buffer` global a managed runtime defines.
//!
//! Every codec here is COMPUTATION over the bytes it is given — UTF-8, hex, base64 (and its
//! URL alphabet), latin-1 and UTF-16 — written out rather than taken from a host library,
//! because the engine this runs in supplies none of them and none of them reaches anything
//! outside the array being converted.

/// `buffer`: `Buffer` as a `Uint8Array` subclass, with the encodings Node names.
pub(super) const BUFFER_SOURCE: &str = r##"
'use strict';

// The two codings that are CODING rather than language — hex and base64, in both its
// alphabets — are computed by the host's own implementations of them
// ([`super::super::digest`]); the character encodings below (UTF-8, latin-1, UTF-16) are
// the text algebra this container holds.
function host() {
    return globalThis.__lambdaRuntime;
}

// The encoding names Node accepts, each mapped to the one codec it names.
var ENCODINGS = {
    'utf8': 'utf8',
    'utf-8': 'utf8',
    'ascii': 'ascii',
    'latin1': 'latin1',
    'binary': 'latin1',
    'hex': 'hex',
    'base64': 'base64',
    'base64url': 'base64url',
    'ucs2': 'utf16le',
    'ucs-2': 'utf16le',
    'utf16le': 'utf16le',
    'utf-16le': 'utf16le'
};

function codecOf(encoding) {
    if (encoding === undefined || encoding === null) {
        return 'utf8';
    }
    var named = ENCODINGS[String(encoding).toLowerCase()];
    if (named === undefined) {
        throw new TypeError('Unknown encoding: ' + encoding);
    }
    return named;
}

function utf8Bytes(text) {
    var out = [];
    for (var index = 0; index < text.length; index += 1) {
        var point = text.charCodeAt(index);
        if (point >= 0xd800 && point <= 0xdbff && index + 1 < text.length) {
            var low = text.charCodeAt(index + 1);
            if (low >= 0xdc00 && low <= 0xdfff) {
                point = 0x10000 + ((point - 0xd800) << 10) + (low - 0xdc00);
                index += 1;
            }
        }
        if (point < 0x80) {
            out.push(point);
        } else if (point < 0x800) {
            out.push(0xc0 | (point >> 6), 0x80 | (point & 0x3f));
        } else if (point < 0x10000) {
            out.push(0xe0 | (point >> 12), 0x80 | ((point >> 6) & 0x3f), 0x80 | (point & 0x3f));
        } else {
            out.push(
                0xf0 | (point >> 18),
                0x80 | ((point >> 12) & 0x3f),
                0x80 | ((point >> 6) & 0x3f),
                0x80 | (point & 0x3f)
            );
        }
    }
    return out;
}

// UTF-8 decoding with the replacement character for every byte sequence that is not one,
// which is what Node's utf8 decoder answers for malformed input.
function utf8Text(bytes, start, end) {
    var out = '';
    var index = start;
    while (index < end) {
        var lead = bytes[index];
        var point = 0xfffd;
        var size = 1;
        if (lead < 0x80) {
            point = lead;
        } else if (lead >= 0xc2 && lead <= 0xdf && index + 1 < end && (bytes[index + 1] & 0xc0) === 0x80) {
            point = ((lead & 0x1f) << 6) | (bytes[index + 1] & 0x3f);
            size = 2;
        } else if (lead >= 0xe0 && lead <= 0xef && index + 2 < end
            && (bytes[index + 1] & 0xc0) === 0x80 && (bytes[index + 2] & 0xc0) === 0x80) {
            point = ((lead & 0x0f) << 12) | ((bytes[index + 1] & 0x3f) << 6) | (bytes[index + 2] & 0x3f);
            size = 3;
            if (point < 0x800 || (point >= 0xd800 && point <= 0xdfff)) {
                point = 0xfffd;
            }
        } else if (lead >= 0xf0 && lead <= 0xf4 && index + 3 < end
            && (bytes[index + 1] & 0xc0) === 0x80 && (bytes[index + 2] & 0xc0) === 0x80
            && (bytes[index + 3] & 0xc0) === 0x80) {
            point = ((lead & 0x07) << 18) | ((bytes[index + 1] & 0x3f) << 12)
                | ((bytes[index + 2] & 0x3f) << 6) | (bytes[index + 3] & 0x3f);
            size = 4;
            if (point < 0x10000 || point > 0x10ffff) {
                point = 0xfffd;
            }
        }
        if (point > 0xffff) {
            point -= 0x10000;
            out += String.fromCharCode(0xd800 + (point >> 10), 0xdc00 + (point & 0x3ff));
        } else {
            out += String.fromCharCode(point);
        }
        index += size;
    }
    return out;
}

function hexBytes(text) {
    return host().decode('hex', String(text));
}

function hexText(bytes, start, end) {
    return host().encode('hex', Array.prototype.slice.call(bytes, start, end));
}

function base64Bytes(text) {
    return host().decode('base64', String(text));
}

function base64Text(bytes, start, end, urlAlphabet) {
    return host().encode(
        urlAlphabet ? 'base64url' : 'base64',
        Array.prototype.slice.call(bytes, start, end)
    );
}

function latin1Bytes(text, mask) {
    var out = [];
    for (var index = 0; index < text.length; index += 1) {
        out.push(text.charCodeAt(index) & mask);
    }
    return out;
}

function latin1Text(bytes, start, end) {
    var out = '';
    for (var index = start; index < end; index += 1) {
        out += String.fromCharCode(bytes[index]);
    }
    return out;
}

function utf16leBytes(text) {
    var out = [];
    for (var index = 0; index < text.length; index += 1) {
        var unit = text.charCodeAt(index);
        out.push(unit & 0xff, (unit >> 8) & 0xff);
    }
    return out;
}

function utf16leText(bytes, start, end) {
    var out = '';
    for (var index = start; index + 1 < end; index += 2) {
        out += String.fromCharCode(bytes[index] | (bytes[index + 1] << 8));
    }
    return out;
}

function encodedBytes(text, codec) {
    var value = String(text);
    if (codec === 'utf8') {
        return utf8Bytes(value);
    }
    if (codec === 'hex') {
        return hexBytes(value);
    }
    if (codec === 'base64' || codec === 'base64url') {
        return base64Bytes(value);
    }
    if (codec === 'latin1') {
        return latin1Bytes(value, 0xff);
    }
    if (codec === 'ascii') {
        return latin1Bytes(value, 0x7f);
    }
    return utf16leBytes(value);
}

function decodedText(bytes, codec, start, end) {
    if (codec === 'utf8') {
        return utf8Text(bytes, start, end);
    }
    if (codec === 'hex') {
        return hexText(bytes, start, end);
    }
    if (codec === 'base64') {
        return base64Text(bytes, start, end, false);
    }
    if (codec === 'base64url') {
        return base64Text(bytes, start, end, true);
    }
    if (codec === 'latin1' || codec === 'ascii') {
        return latin1Text(bytes, start, end);
    }
    return utf16leText(bytes, start, end);
}

function clampIndex(value, fallback, length) {
    var index = value === undefined || value === null ? fallback : Math.trunc(Number(value));
    if (!isFinite(index)) {
        index = fallback;
    }
    if (index < 0) {
        index += length;
    }
    return Math.min(Math.max(index, 0), length);
}

class Buffer extends Uint8Array {
    static from(value, encodingOrOffset, length) {
        if (typeof value === 'string') {
            var bytes = encodedBytes(value, codecOf(encodingOrOffset));
            var held = new Buffer(bytes.length);
            for (var index = 0; index < bytes.length; index += 1) {
                held[index] = bytes[index];
            }
            return held;
        }
        if (value instanceof ArrayBuffer) {
            var view = length === undefined
                ? new Uint8Array(value, encodingOrOffset === undefined ? 0 : encodingOrOffset)
                : new Uint8Array(value, encodingOrOffset, length);
            var over = new Buffer(view.length);
            over.set(view);
            return over;
        }
        if (value !== null && typeof value === 'object'
            && (value instanceof Uint8Array || typeof value.length === 'number')) {
            var copied = new Buffer(value.length);
            for (var at = 0; at < value.length; at += 1) {
                copied[at] = Number(value[at]) & 0xff;
            }
            return copied;
        }
        if (value !== null && typeof value === 'object' && value.type === 'Buffer'
            && Array.isArray(value.data)) {
            return Buffer.from(value.data);
        }
        throw new TypeError(
            'The first argument must be of type string or an instance of Buffer, ArrayBuffer, '
            + 'or Array. Received ' + typeof value
        );
    }

    static of() {
        return Buffer.from(Array.prototype.slice.call(arguments));
    }

    static alloc(size, fill, encoding) {
        var held = new Buffer(Math.max(0, Math.trunc(Number(size)) || 0));
        if (fill !== undefined && fill !== null && fill !== 0) {
            held.fill(fill, 0, held.length, encoding);
        }
        return held;
    }

    static allocUnsafe(size) {
        return Buffer.alloc(size);
    }

    static allocUnsafeSlow(size) {
        return Buffer.alloc(size);
    }

    static isBuffer(value) {
        return value instanceof Buffer;
    }

    static isEncoding(encoding) {
        return typeof encoding === 'string'
            && ENCODINGS[encoding.toLowerCase()] !== undefined;
    }

    static byteLength(value, encoding) {
        if (typeof value === 'string') {
            return encodedBytes(value, codecOf(encoding)).length;
        }
        if (value instanceof ArrayBuffer) {
            return value.byteLength;
        }
        if (value !== null && typeof value === 'object' && typeof value.length === 'number') {
            return value.length;
        }
        throw new TypeError('The "string" argument must be of type string or an instance of '
            + 'Buffer or ArrayBuffer');
    }

    static concat(list, totalLength) {
        if (!Array.isArray(list)) {
            throw new TypeError('The "list" argument must be an instance of Array');
        }
        var total = 0;
        for (var index = 0; index < list.length; index += 1) {
            total += list[index].length;
        }
        if (totalLength !== undefined && totalLength !== null) {
            total = Math.min(total, Math.max(0, Math.trunc(Number(totalLength))));
        }
        var held = Buffer.alloc(total);
        var at = 0;
        for (var part = 0; part < list.length && at < total; part += 1) {
            var source = list[part];
            for (var byte = 0; byte < source.length && at < total; byte += 1) {
                held[at] = source[byte];
                at += 1;
            }
        }
        return held;
    }

    static compare(first, second) {
        return Buffer.prototype.compare.call(first, second);
    }

    toString(encoding, start, end) {
        var codec = codecOf(encoding);
        var from = clampIndex(start, 0, this.length);
        var to = clampIndex(end, this.length, this.length);
        return to <= from ? '' : decodedText(this, codec, from, to);
    }

    toJSON() {
        return { type: 'Buffer', data: Array.prototype.slice.call(this) };
    }

    equals(other) {
        return this.compare(other) === 0;
    }

    compare(other) {
        var shortest = Math.min(this.length, other.length);
        for (var index = 0; index < shortest; index += 1) {
            if (this[index] !== other[index]) {
                return this[index] < other[index] ? -1 : 1;
            }
        }
        if (this.length === other.length) {
            return 0;
        }
        return this.length < other.length ? -1 : 1;
    }

    write(text, offset, length, encoding) {
        var at = 0;
        var room = this.length;
        var named = encoding;
        if (typeof offset === 'string') {
            named = offset;
        } else if (offset !== undefined && offset !== null) {
            at = clampIndex(offset, 0, this.length);
            if (typeof length === 'string') {
                named = length;
                room = this.length - at;
            } else if (length !== undefined && length !== null) {
                room = Math.max(0, Math.trunc(Number(length)));
            } else {
                room = this.length - at;
            }
        }
        var bytes = encodedBytes(text, codecOf(named));
        var written = Math.min(bytes.length, room, this.length - at);
        for (var index = 0; index < written; index += 1) {
            this[at + index] = bytes[index];
        }
        return written;
    }

    fill(value, start, end, encoding) {
        var from = clampIndex(start, 0, this.length);
        var to = clampIndex(end, this.length, this.length);
        var bytes;
        if (typeof value === 'string') {
            bytes = encodedBytes(value, codecOf(encoding));
        } else if (typeof value === 'number') {
            bytes = [Math.trunc(value) & 0xff];
        } else {
            bytes = Array.prototype.slice.call(value);
        }
        if (bytes.length === 0) {
            bytes = [0];
        }
        for (var index = from; index < to; index += 1) {
            this[index] = bytes[(index - from) % bytes.length];
        }
        return this;
    }

    copy(target, targetStart, start, end) {
        var at = clampIndex(targetStart, 0, target.length);
        var from = clampIndex(start, 0, this.length);
        var to = clampIndex(end, this.length, this.length);
        var copied = 0;
        while (from + copied < to && at + copied < target.length) {
            target[at + copied] = this[from + copied];
            copied += 1;
        }
        return copied;
    }

    slice(start, end) {
        return this.subarray(start, end);
    }

    indexOf(value, start, encoding) {
        var needle = typeof value === 'number'
            ? Buffer.from([Math.trunc(value) & 0xff])
            : (typeof value === 'string' ? Buffer.from(value, encoding) : Buffer.from(value));
        var from = clampIndex(start, 0, this.length);
        if (needle.length === 0) {
            return from;
        }
        for (var index = from; index + needle.length <= this.length; index += 1) {
            var matched = true;
            for (var at = 0; at < needle.length; at += 1) {
                if (this[index + at] !== needle[at]) {
                    matched = false;
                    break;
                }
            }
            if (matched) {
                return index;
            }
        }
        return -1;
    }

    includes(value, start, encoding) {
        return this.indexOf(value, start, encoding) !== -1;
    }

    readUInt8(offset) {
        return this[clampIndex(offset, 0, this.length)];
    }

    readUInt16BE(offset) {
        var at = clampIndex(offset, 0, this.length);
        return (this[at] << 8) | this[at + 1];
    }

    readUInt16LE(offset) {
        var at = clampIndex(offset, 0, this.length);
        return this[at] | (this[at + 1] << 8);
    }

    readUInt32BE(offset) {
        var at = clampIndex(offset, 0, this.length);
        return ((this[at] * 0x1000000) + ((this[at + 1] << 16) | (this[at + 2] << 8) | this[at + 3]));
    }

    readUInt32LE(offset) {
        var at = clampIndex(offset, 0, this.length);
        return (this[at] | (this[at + 1] << 8) | (this[at + 2] << 16)) + (this[at + 3] * 0x1000000);
    }

    readInt8(offset) {
        var byte = this.readUInt8(offset);
        return byte > 0x7f ? byte - 0x100 : byte;
    }

    readInt16BE(offset) {
        var unit = this.readUInt16BE(offset);
        return unit > 0x7fff ? unit - 0x10000 : unit;
    }

    readInt16LE(offset) {
        var unit = this.readUInt16LE(offset);
        return unit > 0x7fff ? unit - 0x10000 : unit;
    }

    readInt32BE(offset) {
        var word = this.readUInt32BE(offset);
        return word > 0x7fffffff ? word - 0x100000000 : word;
    }

    readInt32LE(offset) {
        var word = this.readUInt32LE(offset);
        return word > 0x7fffffff ? word - 0x100000000 : word;
    }

    writeUInt8(value, offset) {
        var at = clampIndex(offset, 0, this.length);
        this[at] = Math.trunc(Number(value)) & 0xff;
        return at + 1;
    }

    writeUInt16BE(value, offset) {
        var at = clampIndex(offset, 0, this.length);
        var unit = Math.trunc(Number(value)) & 0xffff;
        this[at] = unit >> 8;
        this[at + 1] = unit & 0xff;
        return at + 2;
    }

    writeUInt16LE(value, offset) {
        var at = clampIndex(offset, 0, this.length);
        var unit = Math.trunc(Number(value)) & 0xffff;
        this[at] = unit & 0xff;
        this[at + 1] = unit >> 8;
        return at + 2;
    }

    writeUInt32BE(value, offset) {
        var at = clampIndex(offset, 0, this.length);
        var word = Math.trunc(Number(value)) >>> 0;
        this[at] = (word >>> 24) & 0xff;
        this[at + 1] = (word >>> 16) & 0xff;
        this[at + 2] = (word >>> 8) & 0xff;
        this[at + 3] = word & 0xff;
        return at + 4;
    }

    writeUInt32LE(value, offset) {
        var at = clampIndex(offset, 0, this.length);
        var word = Math.trunc(Number(value)) >>> 0;
        this[at] = word & 0xff;
        this[at + 1] = (word >>> 8) & 0xff;
        this[at + 2] = (word >>> 16) & 0xff;
        this[at + 3] = (word >>> 24) & 0xff;
        return at + 4;
    }

    // The 64-bit readings and writings, which carry a `BigInt` because a 64-bit value does
    // not fit a double exactly — the length field of a framed message, a sequence number, a
    // timestamp in nanoseconds. Each is the same byte order as its 32-bit sibling above.
    readBigUInt64BE(offset) {
        var at = clampIndex(offset, 0, this.length);
        var value = BigInt(0);
        for (var index = 0; index < 8; index += 1) {
            value = (value << BigInt(8)) | BigInt(this[at + index] & 0xff);
        }
        return value;
    }

    readBigUInt64LE(offset) {
        var at = clampIndex(offset, 0, this.length);
        var value = BigInt(0);
        for (var index = 7; index >= 0; index -= 1) {
            value = (value << BigInt(8)) | BigInt(this[at + index] & 0xff);
        }
        return value;
    }

    readBigInt64BE(offset) {
        var value = this.readBigUInt64BE(offset);
        return value > BigInt('0x7fffffffffffffff')
            ? value - BigInt('0x10000000000000000')
            : value;
    }

    readBigInt64LE(offset) {
        var value = this.readBigUInt64LE(offset);
        return value > BigInt('0x7fffffffffffffff')
            ? value - BigInt('0x10000000000000000')
            : value;
    }

    writeBigUInt64BE(value, offset) {
        var at = clampIndex(offset, 0, this.length);
        var held = BigInt(value) & BigInt('0xffffffffffffffff');
        for (var index = 7; index >= 0; index -= 1) {
            this[at + index] = Number(held & BigInt(0xff));
            held = held >> BigInt(8);
        }
        return at + 8;
    }

    writeBigUInt64LE(value, offset) {
        var at = clampIndex(offset, 0, this.length);
        var held = BigInt(value) & BigInt('0xffffffffffffffff');
        for (var index = 0; index < 8; index += 1) {
            this[at + index] = Number(held & BigInt(0xff));
            held = held >> BigInt(8);
        }
        return at + 8;
    }

    writeBigInt64BE(value, offset) {
        return this.writeBigUInt64BE(value, offset);
    }

    writeBigInt64LE(value, offset) {
        return this.writeBigUInt64LE(value, offset);
    }
}

// `Buffer.poolSize` is read by programs that size their own reads by it; this container
// allocates every buffer on its own, so the pool is the one allocation it makes.
Buffer.poolSize = 8192;

exports.Buffer = Buffer;
exports.kMaxLength = 0x7fffffff;
exports.kStringMaxLength = 0x1fffffe8;
exports.constants = { MAX_LENGTH: 0x7fffffff, MAX_STRING_LENGTH: 0x1fffffe8 };
exports.atob = function (text) {
    var bytes = base64Bytes(String(text));
    return latin1Text(bytes, 0, bytes.length);
};
exports.btoa = function (text) {
    var bytes = latin1Bytes(String(text), 0xff);
    return base64Text(bytes, 0, bytes.length, false);
};
exports.isUtf8 = function (bytes) {
    return utf8Text(bytes, 0, bytes.length).indexOf('\ufffd') === -1;
};
exports.isAscii = function (bytes) {
    for (var index = 0; index < bytes.length; index += 1) {
        if (bytes[index] > 0x7f) {
            return false;
        }
    }
    return true;
};
exports.transcode = function (source, from, to) {
    return Buffer.from(source.toString(from), to);
};
exports.SlowBuffer = function (size) {
    return Buffer.alloc(size);
};
"##;
