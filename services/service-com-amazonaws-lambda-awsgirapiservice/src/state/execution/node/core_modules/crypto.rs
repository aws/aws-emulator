// SPDX-License-Identifier: CC0-1.0

//! `crypto`: the digests, the keyed digests, the symmetric ciphers and the random bytes a
//! Node program asks its runtime for.
//!
//! The arithmetic is NOT written here: a digest and a keyed digest are computed by the host
//! ([`super::super::digest`]), a symmetric transform by the host's own cipher seam
//! ([`super::super::cipher`]), and random bytes come from the host's own cryptographic
//! source. This module is the Node-shaped interface over them: it holds the bytes a program
//! feeds a hash or a cipher, names the algorithm, and hands the answer back in the encoding
//! the program asked for. Nothing here reaches a host file, socket or environment.

/// `crypto`: hashes, HMACs, symmetric ciphers and random bytes.
pub(super) const CRYPTO_SOURCE: &str = r##"
'use strict';

var Buffer = require('buffer').Buffer;

// The host half of this module: the three pure computations the runtime protocol exposes.
function host() {
    return globalThis.__lambdaRuntime;
}

// The digest algorithms this runtime computes, by the name a Node program names them
// under. One per implementation the host links; an algorithm that is not here is refused
// rather than answered with a digest of another algorithm.
var ALGORITHMS = ['md5', 'sha1', 'sha256', 'sha384', 'sha512'];

// How many bytes one draw of random may ask for, which is the ceiling the HOST enforces
// ([`super::super::digest::MAX_RANDOM_BYTES`]): the draw happens in host memory, outside
// the engine's own allocation ceiling, so the bound is stated on both sides of the call.
var MAX_RANDOM_BYTES = 65536;

// The length of each digest this runtime computes, in bytes, which is what bounds a key
// derived from it: the extract-and-expand construction derives at most 255 times the
// digest's own length (RFC 5869 §2.3). The bound is stated on both sides of the host call,
// as the random draw's is above and for the same reason.
var DIGEST_BYTES = { md5: 16, sha1: 20, sha256: 32, sha384: 48, sha512: 64 };

function algorithmOf(name) {
    var wanted = String(name).toLowerCase().replace(/-/g, '');
    for (var index = 0; index < ALGORITHMS.length; index += 1) {
        if (ALGORITHMS[index] === wanted) {
            return ALGORITHMS[index];
        }
    }
    var error = new Error('Digest method not supported');
    error.code = 'ERR_CRYPTO_INVALID_DIGEST';
    throw error;
}

function hexOf(data, encoding) {
    if (typeof data === 'string') {
        return Buffer.from(data, encoding === undefined ? 'utf8' : encoding).toString('hex');
    }
    if (data === null || data === undefined) {
        throw new TypeError('The "data" argument must be of type string or an instance of '
            + 'Buffer, TypedArray, or DataView');
    }
    return Buffer.from(data).toString('hex');
}

function encodedDigest(hex, encoding) {
    var bytes = Buffer.from(hex, 'hex');
    return encoding === undefined || encoding === null ? bytes : bytes.toString(encoding);
}

function Hash(algorithm) {
    this.algorithm = algorithmOf(algorithm);
    this.parts = '';
    this.done = false;
}

Hash.prototype.update = function (data, encoding) {
    if (this.done) {
        throw new Error('Digest already called');
    }
    this.parts += hexOf(data, encoding);
    return this;
};

Hash.prototype.digest = function (encoding) {
    if (this.done) {
        throw new Error('Digest already called');
    }
    this.done = true;
    return encodedDigest(host().digest(this.algorithm, this.parts), encoding);
};

Hash.prototype.copy = function () {
    var copied = new Hash(this.algorithm);
    copied.parts = this.parts;
    return copied;
};

function Hmac(algorithm, key) {
    this.algorithm = algorithmOf(algorithm);
    this.key = hexOf(key);
    this.parts = '';
    this.done = false;
}

Hmac.prototype.update = function (data, encoding) {
    if (this.done) {
        throw new Error('Digest already called');
    }
    this.parts += hexOf(data, encoding);
    return this;
};

Hmac.prototype.digest = function (encoding) {
    if (this.done) {
        throw new Error('Digest already called');
    }
    this.done = true;
    return encodedDigest(host().hmac(this.algorithm, this.key, this.parts), encoding);
};

function randomBytes(size, callback) {
    var count = Math.trunc(Number(size));
    // The size one draw may ask for. Node's own ceiling is `buffer.kMaxLength`, and a
    // request past it is out of range rather than an allocation: the bytes are materialised
    // outside the engine's own heap, so the request is refused before anything is drawn.
    if (!Number.isSafeInteger(count) || count < 0 || count > MAX_RANDOM_BYTES) {
        var error = new RangeError('The value of "size" is out of range. It must be >= 0 && <= '
            + MAX_RANDOM_BYTES + '. Received ' + String(size));
        error.code = 'ERR_OUT_OF_RANGE';
        throw error;
    }
    var bytes = Buffer.from(host().randomHex(count), 'hex');
    if (typeof callback === 'function') {
        Promise.resolve().then(function () { callback(null, bytes); });
        return undefined;
    }
    return bytes;
}

function randomFillSync(target, offset, size) {
    var at = offset === undefined ? 0 : Math.trunc(Number(offset));
    var count = size === undefined ? target.length - at : Math.trunc(Number(size));
    var bytes = randomBytes(count);
    for (var index = 0; index < count; index += 1) {
        target[at + index] = bytes[index];
    }
    return target;
}

// A v4 UUID: 122 random bits with the version and variant bits the format fixes.
function randomUUID() {
    var bytes = randomBytes(16);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    var hex = bytes.toString('hex');
    return hex.slice(0, 8) + '-' + hex.slice(8, 12) + '-' + hex.slice(12, 16) + '-'
        + hex.slice(16, 20) + '-' + hex.slice(20);
}

// An integer of [min, max), drawn by rejection so every value of the range is equally
// likely however wide the range is.
function randomInt(min, max, callback) {
    var low = max === undefined || typeof max === 'function' ? 0 : Math.trunc(Number(min));
    var high = max === undefined || typeof max === 'function'
        ? Math.trunc(Number(min))
        : Math.trunc(Number(max));
    var done = typeof max === 'function' ? max : callback;
    if (!(high > low)) {
        throw new RangeError('The value of "max" is out of range.');
    }
    var span = high - low;
    var bytes = 0;
    var ceiling = 1;
    while (ceiling < span) {
        ceiling *= 256;
        bytes += 1;
    }
    var drawn = span;
    while (drawn >= ceiling - (ceiling % span)) {
        drawn = 0;
        var held = randomBytes(bytes);
        for (var index = 0; index < bytes; index += 1) {
            drawn = (drawn * 256) + held[index];
        }
    }
    var value = low + (drawn % span);
    if (typeof done === 'function') {
        Promise.resolve().then(function () { done(null, value); });
        return undefined;
    }
    return value;
}

function timingSafeEqual(first, second) {
    var left = Buffer.from(first);
    var right = Buffer.from(second);
    if (left.length !== right.length) {
        throw new RangeError('Input buffers must have the same byte length');
    }
    var difference = 0;
    for (var index = 0; index < left.length; index += 1) {
        difference |= left[index] ^ right[index];
    }
    return difference === 0;
}

function getRandomValues(target) {
    return randomFillSync(target, 0, target.length);
}

// The ciphers this runtime computes, by the name a Node program names them under. The list
// is the HOST's own ([`super::super::cipher::CIPHERS`]) rather than a second copy of it, so
// what `getCiphers()` reports and what a call can compute cannot drift apart; it is read on
// first use because a module of this runtime is evaluated before any of it is called.
var HELD_CIPHERS = null;

function ciphers() {
    if (HELD_CIPHERS === null) {
        HELD_CIPHERS = host().ciphers();
    }
    return HELD_CIPHERS;
}

function cipherNameOf(name) {
    var wanted = String(name).toLowerCase();
    var named = ciphers();
    for (var index = 0; index < named.length; index += 1) {
        if (named[index] === wanted) {
            return named[index];
        }
    }
    var error = new TypeError('Unknown cipher');
    error.code = 'ERR_CRYPTO_UNKNOWN_CIPHER';
    throw error;
}

// The error one of the host's refusals is reported as, worded the way Node reports that
// failure: the message a caller matches on is the failure's, not this runtime's.
function cipherFailure(outcome, algorithm) {
    var error;
    if (outcome === 'keylen') {
        error = new RangeError('Invalid key length');
        error.code = 'ERR_CRYPTO_INVALID_KEYLEN';
        return error;
    }
    if (outcome === 'iv') {
        error = new TypeError('Invalid initialization vector');
        error.code = 'ERR_CRYPTO_INVALID_IV';
        return error;
    }
    if (outcome === 'unopened') {
        if (algorithm.slice(-4) === '-gcm') {
            return new Error('Unsupported state or unable to authenticate data');
        }
        error = new Error('error:1C800064:Provider routines::bad decrypt');
        error.code = 'ERR_OSSL_BAD_DECRYPT';
        return error;
    }
    if (outcome === 'toolong') {
        error = new RangeError('The value of "data" is out of range. It must be at most '
            + MAX_CIPHER_BYTES + ' bytes');
        error.code = 'ERR_OUT_OF_RANGE';
        return error;
    }
    if (outcome === 'malformed') {
        error = new TypeError('The "data" argument must be of type string or an instance of '
            + 'Buffer, TypedArray, or DataView');
        error.code = 'ERR_INVALID_ARG_TYPE';
        return error;
    }
    error = new TypeError('Unknown cipher');
    error.code = 'ERR_CRYPTO_UNKNOWN_CIPHER';
    return error;
}

// How many bytes one transform may carry, which is the ceiling the HOST enforces
// ([`super::super::cipher::MAX_CIPHER_BYTES`]): the bytes are transformed in host memory,
// outside the engine's own allocation ceiling, so the bound is named on both sides of the
// call as the random draw's is.
var MAX_CIPHER_BYTES = 8388608;

// One cipher in one direction: the key, the IV and the bytes a program feeds it.
//
// A COUNTER mode is a stream, so the transform of what has arrived already is settled and
// `update` hands those bytes over, as a node cipher does. The other two modes settle when the
// message does: a padded mode's last block carries the padding, and an authenticated mode's
// message is not known to open until its tag is verified — so their bytes are handed over at
// `final`, and the concatenation of what `update` and `final` answer is the whole transform
// either way.
function Cipher(algorithm, key, iv, decrypting) {
    this.algorithm = cipherNameOf(algorithm);
    this.keyHex = hexOf(key);
    this.ivHex = iv === null || iv === undefined ? '' : hexOf(iv);
    this.decrypting = decrypting === true;
    this.streamed = this.algorithm.slice(-4) === '-ctr';
    this.aadHex = '';
    this.partsHex = '';
    this.tagHex = '';
    this.authTag = null;
    this.handedOver = 0;
    this.done = false;
}

Cipher.prototype.setAAD = function (data, options) {
    // Additional data is part of an AUTHENTICATED mode's own computation; a mode that
    // authenticates nothing has nowhere to carry it, so being given some is a state this
    // cipher cannot be in rather than something to ignore.
    if (this.done || this.algorithm.slice(-4) !== '-gcm') {
        var error = new Error('Invalid state for operation setAAD');
        error.code = 'ERR_CRYPTO_INVALID_STATE';
        throw error;
    }
    this.aadHex += hexOf(data, options === undefined || options === null
        ? undefined
        : options.encoding);
    return this;
};

Cipher.prototype.setAutoPadding = function (auto) {
    // The padded mode is the only one padding decides anything for, and the transform this
    // runtime computes is the padded one: turning it off is refused rather than ignored.
    if (auto === false && this.algorithm.slice(-4) === '-cbc') {
        var error = new Error('Unsupported state or unable to authenticate data');
        error.code = 'ERR_CRYPTO_UNSUPPORTED_OPERATION';
        throw error;
    }
    return this;
};

Cipher.prototype.setAuthTag = function (tag) {
    if (this.done) {
        throw new Error('Unsupported state');
    }
    this.tagHex = hexOf(tag);
    return this;
};

Cipher.prototype.getAuthTag = function () {
    if (this.authTag === null) {
        throw new Error('Unsupported state');
    }
    return this.authTag;
};

// The whole transform of the bytes fed so far, as the host computes it; a refusal is thrown
// as the error that failure is. An encryption's tag is held as it becomes known.
Cipher.prototype.transformed = function () {
    var answered = host().cipher(
        [this.algorithm, this.keyHex, this.ivHex, this.aadHex, this.partsHex, this.tagHex],
        this.decrypting
    );
    if (answered[0] !== 'ok') {
        throw cipherFailure(answered[0], this.algorithm);
    }
    if (!this.decrypting) {
        this.authTag = Buffer.from(answered[2], 'hex');
    }
    return Buffer.from(answered[1], 'hex');
};

// The bytes of `whole` this cipher has not handed over yet.
Cipher.prototype.notYetHandedOver = function (whole) {
    var from = this.handedOver < whole.length ? this.handedOver : whole.length;
    this.handedOver = whole.length;
    return Buffer.from(whole.subarray(from));
};

// `bytes` in the form the caller asked for: the bytes themselves, or their text under the
// encoding named.
function cipherOutput(bytes, outputEncoding) {
    return outputEncoding === undefined || outputEncoding === null
        ? bytes
        : bytes.toString(outputEncoding);
}

Cipher.prototype.update = function (data, inputEncoding, outputEncoding) {
    if (this.done) {
        throw new Error('Trying to add data in unsupported state');
    }
    this.partsHex += hexOf(data, inputEncoding === null ? undefined : inputEncoding);
    var bytes = this.streamed
        ? this.notYetHandedOver(this.transformed())
        : Buffer.alloc(0);
    return cipherOutput(bytes, outputEncoding);
};

Cipher.prototype.final = function (outputEncoding) {
    if (this.done) {
        throw new Error('Unsupported state');
    }
    this.done = true;
    return cipherOutput(this.notYetHandedOver(this.transformed()), outputEncoding);
};

var webcrypto = {
    getRandomValues: getRandomValues,
    randomUUID: randomUUID,
    subtle: {
        digest: function (algorithm, data) {
            var named = typeof algorithm === 'string' ? algorithm : algorithm.name;
            var hashed = new Hash(named).update(Buffer.from(data)).digest();
            return Promise.resolve(
                hashed.buffer.slice(hashed.byteOffset, hashed.byteOffset + hashed.length)
            );
        }
    }
};

// The bytes `value` holds, as the ArrayBuffer a WebCrypto call answers with.
function arrayBufferOf(bytes) {
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.length);
}

// The cipher one WebCrypto algorithm names, for a key of that many bytes: the two names
// speak of the same transforms, so one table maps between them.
function webCipherName(algorithm, keyBytes) {
    var named = String(typeof algorithm === 'string' ? algorithm : algorithm.name).toUpperCase();
    var bits = keyBytes * 8;
    if (named === 'AES-GCM' || named === 'AES-CBC' || named === 'AES-CTR') {
        return 'aes-' + bits + '-' + named.slice('AES-'.length).toLowerCase();
    }
    var error = new Error('Unrecognized algorithm name');
    error.name = 'NotSupportedError';
    throw error;
}

// A key a WebCrypto call holds: the raw bytes, with what the key may be used for. The bytes
// are held on the key and nowhere else, which is what `exportKey` reads back.
function SubtleKey(algorithm, extractable, usages, bytes) {
    this.type = 'secret';
    this.algorithm = typeof algorithm === 'string' ? { name: algorithm } : algorithm;
    this.extractable = extractable !== false;
    this.usages = usages === undefined || usages === null ? [] : usages;
    this.keyBytes = bytes;
}

// The nonce or counter a WebCrypto AES call carries, by the member that mode names it in.
function webCipherIv(algorithm, cipher) {
    if (cipher.slice(-4) === '-ctr') {
        return Buffer.from(algorithm.counter);
    }
    return Buffer.from(algorithm.iv);
}

// How long the tag of an authenticated WebCrypto call is, in bytes: the mode's full tag,
// which is the only length this runtime computes.
var WEB_TAG_BYTES = 16;

function webTransform(algorithm, key, data, decrypting) {
    var cipher = webCipherName(algorithm, key.keyBytes.length);
    var authenticated = cipher.slice(-4) === '-gcm';
    if (authenticated
        && algorithm.tagLength !== undefined
        && Number(algorithm.tagLength) !== WEB_TAG_BYTES * 8) {
        var unsupported = new Error('Unsupported tag length');
        unsupported.name = 'NotSupportedError';
        return Promise.reject(unsupported);
    }
    var bytes = Buffer.from(data);
    var tag = Buffer.alloc(0);
    // A WebCrypto ciphertext carries the tag of an authenticated mode APPENDED to it, where
    // the node cipher API hands it over separately.
    if (authenticated && decrypting) {
        if (bytes.length < WEB_TAG_BYTES) {
            var short = new Error('The operation failed for an operation-specific reason');
            short.name = 'OperationError';
            return Promise.reject(short);
        }
        tag = Buffer.from(bytes.subarray(bytes.length - WEB_TAG_BYTES));
        bytes = Buffer.from(bytes.subarray(0, bytes.length - WEB_TAG_BYTES));
    }
    var aad = authenticated && algorithm.additionalData !== undefined
        && algorithm.additionalData !== null
        ? Buffer.from(algorithm.additionalData)
        : Buffer.alloc(0);
    var answered = host().cipher(
        [
            cipher,
            Buffer.from(key.keyBytes).toString('hex'),
            webCipherIv(algorithm, cipher).toString('hex'),
            aad.toString('hex'),
            bytes.toString('hex'),
            tag.toString('hex')
        ],
        decrypting
    );
    if (answered[0] !== 'ok') {
        var failed = cipherFailure(answered[0], cipher);
        // WebCrypto reports every failure of the transform itself as one error class,
        // whatever the node API's own name for it is.
        if (answered[0] === 'unopened') {
            failed = new Error('The operation failed for an operation-specific reason');
            failed.name = 'OperationError';
        }
        return Promise.reject(failed);
    }
    var out = Buffer.from(answered[1], 'hex');
    if (authenticated && !decrypting) {
        out = Buffer.concat([out, Buffer.from(answered[2], 'hex')]);
    }
    return Promise.resolve(arrayBufferOf(out));
}

webcrypto.subtle.importKey = function (format, keyData, algorithm, extractable, usages) {
    if (String(format) !== 'raw') {
        var unsupported = new Error('Unsupported key format');
        unsupported.name = 'NotSupportedError';
        return Promise.reject(unsupported);
    }
    var bytes = Buffer.from(keyData);
    try {
        webCipherName(algorithm, bytes.length);
    } catch (error) {
        return Promise.reject(error);
    }
    return Promise.resolve(new SubtleKey(algorithm, extractable, usages, bytes));
};

webcrypto.subtle.exportKey = function (format, key) {
    if (String(format) !== 'raw') {
        var unsupported = new Error('Unsupported key format');
        unsupported.name = 'NotSupportedError';
        return Promise.reject(unsupported);
    }
    if (key.extractable === false) {
        var refused = new Error('key is not extractable');
        refused.name = 'InvalidAccessError';
        return Promise.reject(refused);
    }
    return Promise.resolve(arrayBufferOf(Buffer.from(key.keyBytes)));
};

webcrypto.subtle.encrypt = function (algorithm, key, data) {
    try {
        return webTransform(algorithm, key, data, false);
    } catch (error) {
        return Promise.reject(error);
    }
};

webcrypto.subtle.decrypt = function (algorithm, key, data) {
    try {
        return webTransform(algorithm, key, data, true);
    } catch (error) {
        return Promise.reject(error);
    }
};

exports.createHash = function (algorithm) { return new Hash(algorithm); };
exports.createHmac = function (algorithm, key) { return new Hmac(algorithm, key); };
exports.createCipheriv = function (algorithm, key, iv) {
    return new Cipher(algorithm, key, iv, false);
};
exports.createDecipheriv = function (algorithm, key, iv) {
    return new Cipher(algorithm, key, iv, true);
};
exports.getCiphers = function () { return ciphers().slice(); };
exports.hkdfSync = function (digest, ikm, salt, info, keylen) {
    var algorithm = algorithmOf(digest);
    var ceiling = 255 * DIGEST_BYTES[algorithm];
    var length = Math.trunc(Number(keylen));
    if (!Number.isSafeInteger(length) || length < 0 || length > ceiling) {
        var error = new RangeError('The value of "length" is out of range. It must be >= 0 && <= '
            + ceiling + '. Received ' + String(keylen));
        error.code = 'ERR_OUT_OF_RANGE';
        throw error;
    }
    var derived = host().hkdf(
        [algorithm, hexOf(ikm), hexOf(salt), hexOf(info)],
        length
    );
    // The construction answers bytes, and this call answers the ArrayBuffer Node's own
    // `hkdfSync` hands back.
    return arrayBufferOf(Buffer.from(derived, 'hex'));
};
exports.hkdf = function (digest, ikm, salt, info, keylen, callback) {
    var done = typeof callback === 'function' ? callback : function () { return undefined; };
    var derived = null;
    try {
        derived = exports.hkdfSync(digest, ikm, salt, info, keylen);
    } catch (error) {
        Promise.resolve().then(function () { done(error); });
        return undefined;
    }
    Promise.resolve().then(function () { done(null, derived); });
    return undefined;
};
exports.Cipheriv = Cipher;
exports.Decipheriv = Cipher;
exports.Hash = Hash;
exports.Hmac = Hmac;
exports.getHashes = function () { return ALGORITHMS.slice(); };
exports.randomBytes = randomBytes;
exports.pseudoRandomBytes = randomBytes;
exports.randomFillSync = randomFillSync;
exports.randomFill = function (target, offset, size, callback) {
    var done = typeof offset === 'function' ? offset
        : (typeof size === 'function' ? size : callback);
    var at = typeof offset === 'function' ? 0 : offset;
    var count = typeof size === 'function' || size === undefined ? undefined : size;
    var filled = randomFillSync(target, at, count);
    Promise.resolve().then(function () { done(null, filled); });
};
exports.randomUUID = randomUUID;
exports.randomInt = randomInt;
exports.timingSafeEqual = timingSafeEqual;
exports.getRandomValues = getRandomValues;
exports.webcrypto = webcrypto;
exports.subtle = webcrypto.subtle;
exports.constants = {};
"##;
