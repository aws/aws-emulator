// SPDX-License-Identifier: CC0-1.0

//! The Amazon Redshift SQL functions this emulator's engine does not carry, as
//! real implementations registered on every database connection.
//!
//! The Data API runs whatever SQL its data warehouse accepts, and a Redshift
//! statement reaches for Redshift's function library: `MD5` and the `SHA`
//! family, `LEFT`/`RIGHT`/`REPEAT`, `LEN`, `STRPOS`, `SPLIT_PART`, `INITCAP`,
//! `BTRIM`, `NVL`. The engine behind this service implements the SQL core and
//! its own function set, so each function named here is COMPUTED — hashed,
//! sliced, counted — by an implementation installed on the connection before
//! any caller SQL reaches it, never pretended at by rewriting the call into
//! something else.
//!
//! Each one follows the function it emulates: a `NULL` argument makes the result
//! `NULL` (the whole family is strict), a character count is counted in
//! CHARACTERS rather than bytes, and a position is 1-based with 0 meaning "not
//! found". A result the engine's own value ceiling
//! ([`super::MAX_VALUE_BYTES`]) cannot hold is an error rather than a value cut
//! short.
//!
//! The functions the engine ALREADY carries under the same name and meaning —
//! `LENGTH`, `SUBSTR`, `UPPER`, `LOWER`, `TRIM`, `REPLACE`, `COALESCE`, `ABS`,
//! `ROUND`, `RANDOM` — are not re-registered: the engine's own are the
//! implementations, and shadowing them would replace a tested implementation
//! with another.

use std::time::SystemTime;

use rusqlite::functions::{Context, FunctionFlags};
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, Error, Result};

/// Register every function this module implements on `connection`.
///
/// Called from [`super::Database::open`], so a function is there before the
/// connection ever sees caller SQL.
pub fn install(connection: &Connection) -> Result<()> {
    // Deterministic: the same arguments give the same answer, so the engine may
    // hoist the call out of a loop. The clock-reading functions below are NOT
    // declared deterministic.
    let pure = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;

    // Hash functions. Amazon Redshift's `MD5`, `SHA`/`SHA1` and `SHA2` return
    // the hash as a lower-case hexadecimal string.
    connection.create_scalar_function("md5", 1, pure, |ctx| {
        hashed(ctx, |bytes| hex::encode(<md5::Md5 as md5::Digest>::digest(bytes)))
    })?;
    connection.create_scalar_function("sha", 1, pure, |ctx| {
        hashed(ctx, |bytes| hex::encode(<sha1::Sha1 as sha1::Digest>::digest(bytes)))
    })?;
    connection.create_scalar_function("sha1", 1, pure, |ctx| {
        hashed(ctx, |bytes| hex::encode(<sha1::Sha1 as sha1::Digest>::digest(bytes)))
    })?;
    // "SHA2 … An integer that specifies the number of bits in the hash
    // functions. Valid values are 0, 224, 256, 384, and 512. 0 is equivalent to
    // 256."
    connection.create_scalar_function("sha2", 2, pure, |ctx| {
        let Some(bytes) = raw_argument(ctx, 0) else {
            return Ok(None);
        };
        let Some(bits) = integer_argument(ctx, 1)? else {
            return Ok(None);
        };
        let bytes = bytes.as_slice();
        let digest = match bits {
            0 | 256 => hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bytes)),
            224 => hex::encode(<sha2::Sha224 as sha2::Digest>::digest(bytes)),
            384 => hex::encode(<sha2::Sha384 as sha2::Digest>::digest(bytes)),
            512 => hex::encode(<sha2::Sha512 as sha2::Digest>::digest(bytes)),
            other => return Err(failed(format!("invalid number of bits for SHA2: {other}"))),
        };
        Ok(Some(digest))
    })?;

    // String functions.
    connection.create_scalar_function("len", 1, pure, |ctx| {
        Ok(text_argument(ctx, 0)?.map(|text| i64::try_from(text.chars().count()).unwrap_or(i64::MAX)))
    })?;
    connection.create_scalar_function("left", 2, pure, |ctx| {
        sliced(ctx, Side::Left)
    })?;
    connection.create_scalar_function("right", 2, pure, |ctx| {
        sliced(ctx, Side::Right)
    })?;
    connection.create_scalar_function("repeat", 2, pure, repeated)?;
    // "REPLICATE … is a synonym of the REPEAT function."
    connection.create_scalar_function("replicate", 2, pure, repeated)?;
    connection.create_scalar_function("reverse", 1, pure, |ctx| {
        Ok(text_argument(ctx, 0)?.map(|text| text.chars().rev().collect::<String>()))
    })?;
    // "STRPOS … Returns the position of a substring within a specified string",
    // 1-based, and 0 when the substring is not there.
    connection.create_scalar_function("strpos", 2, pure, |ctx| position(ctx, 0, 1))?;
    // "CHARINDEX … takes the substring first", which is the only difference.
    connection.create_scalar_function("charindex", 2, pure, |ctx| position(ctx, 1, 0))?;
    connection.create_scalar_function("split_part", 3, pure, split_part)?;
    connection.create_scalar_function("initcap", 1, pure, |ctx| {
        Ok(text_argument(ctx, 0)?.map(|text| initcap(&text)))
    })?;
    // "BTRIM … Trims a string by removing leading and trailing blanks or by
    // removing characters that match an optional specified string." The engine
    // carries `TRIM`, `LTRIM` and `RTRIM` under those names and not this one, so
    // this is the one of the family that has to be implemented. Each ARITY the
    // function takes is registered on its own, so the engine itself refuses a
    // call with any other number of arguments — as the real function does —
    // rather than this implementation reading an argument that is not there.
    connection.create_scalar_function("btrim", 1, pure, |ctx| {
        Ok(text_argument(ctx, 0)?.map(|text| text.trim().to_string()))
    })?;
    connection.create_scalar_function("btrim", 2, pure, |ctx| {
        let Some(text) = text_argument(ctx, 0)? else {
            return Ok(None);
        };
        let Some(characters) = text_argument(ctx, 1)? else {
            return Ok(None);
        };
        let trimmed: &str = text.trim_matches(|character| characters.contains(character));
        Ok(Some(trimmed.to_string()))
    })?;

    // Conditional expressions. "NVL … is a synonym for COALESCE" and takes two
    // or more expressions, so it is variadic like the function it mirrors.
    connection.create_scalar_function("nvl", -1, pure, |ctx| {
        for index in 0..ctx.len() {
            let value = ctx.get_raw(index);
            if !matches!(value, ValueRef::Null) {
                return Ok(owned(value));
            }
        }
        Ok(Value::Null)
    })?;
    // "NVL2 … returns the second expression if the first is not null, and the
    // third if it is."
    connection.create_scalar_function("nvl2", 3, pure, |ctx| {
        let decided = if matches!(ctx.get_raw(0), ValueRef::Null) {
            ctx.get_raw(2)
        } else {
            ctx.get_raw(1)
        };
        Ok(owned(decided))
    })?;

    // The clock. "GETDATE … returns the current date and time in the current
    // session time zone", and PostgreSQL's `NOW()` is the same reading; neither
    // is deterministic, so neither is declared so.
    let clock = FunctionFlags::SQLITE_UTF8;
    connection.create_scalar_function("getdate", 0, clock, |_ctx| Ok(timestamp_now()))?;
    connection.create_scalar_function("now", 0, clock, |_ctx| Ok(timestamp_now()))?;

    Ok(())
}

/// Which end of a string [`sliced`] keeps.
#[derive(Debug, Clone, Copy)]
enum Side {
    /// `LEFT`: the leftmost characters.
    Left,
    /// `RIGHT`: the rightmost characters.
    Right,
}

/// `LEFT`/`RIGHT`: "return the specified number of leftmost or rightmost
/// characters from a character string". A count at or below zero asks for no
/// characters.
fn sliced(ctx: &Context<'_>, side: Side) -> Result<Option<String>> {
    let (Some(text), Some(count)) = (text_argument(ctx, 0)?, integer_argument(ctx, 1)?) else {
        return Ok(None);
    };
    let wanted = usize::try_from(count.max(0)).unwrap_or(0);
    let sliced = match side {
        Side::Left => text.chars().take(wanted).collect::<String>(),
        Side::Right => {
            let total = text.chars().count();
            text.chars().skip(total.saturating_sub(wanted)).collect()
        }
    };
    Ok(Some(sliced))
}

/// `REPEAT`: "repeats a string the specified number of times". A count at or
/// below zero repeats it no times; a result the engine's value ceiling cannot
/// hold is an error rather than a string cut short.
fn repeated(ctx: &Context<'_>) -> Result<Option<String>> {
    let (Some(text), Some(count)) = (text_argument(ctx, 0)?, integer_argument(ctx, 1)?) else {
        return Ok(None);
    };
    let times = usize::try_from(count.max(0)).unwrap_or(0);
    let ceiling = usize::try_from(super::MAX_VALUE_BYTES).unwrap_or(usize::MAX);
    if text.len().saturating_mul(times) > ceiling {
        return Err(failed(format!(
            "string or blob too big: a repeated string may not exceed {ceiling} bytes"
        )));
    }
    Ok(Some(text.repeat(times)))
}

/// `STRPOS`/`CHARINDEX`: the 1-based character position of `substring` in
/// `string`, or 0 when it does not occur in it.
fn position(ctx: &Context<'_>, string: usize, substring: usize) -> Result<Option<i64>> {
    let (Some(text), Some(wanted)) = (
        text_argument(ctx, string)?,
        text_argument(ctx, substring)?,
    ) else {
        return Ok(None);
    };
    let found = match text.find(&wanted) {
        None => 0,
        Some(byte) => i64::try_from(text[..byte].chars().count() + 1).unwrap_or(i64::MAX),
    };
    Ok(Some(found))
}

/// `SPLIT_PART`: "splits a string on the specified delimiter and returns the
/// part at the specified position", counting from 1, and the empty string for a
/// position past the parts the string has.
fn split_part(ctx: &Context<'_>) -> Result<Option<String>> {
    let (Some(text), Some(delimiter), Some(part)) = (
        text_argument(ctx, 0)?,
        text_argument(ctx, 1)?,
        integer_argument(ctx, 2)?,
    ) else {
        return Ok(None);
    };
    if part < 1 {
        return Err(failed("field position must be greater than zero".to_string()));
    }
    let index = usize::try_from(part - 1).unwrap_or(usize::MAX);
    // An empty delimiter splits nothing: the whole string is its first part.
    if delimiter.is_empty() {
        return Ok(Some(if index == 0 { text } else { String::new() }));
    }
    Ok(Some(
        text.split(delimiter.as_str())
            .nth(index)
            .unwrap_or_default()
            .to_string(),
    ))
}

/// `INITCAP`: "capitalizes the first letter of each word … and converts any
/// remaining characters to lowercase"; a word is delimited by any character
/// that is not a letter or a digit.
fn initcap(text: &str) -> String {
    let mut capitalised = String::with_capacity(text.len());
    let mut starting = true;
    for character in text.chars() {
        if !character.is_alphanumeric() {
            capitalised.push(character);
            starting = true;
            continue;
        }
        if starting {
            capitalised.extend(character.to_uppercase());
        } else {
            capitalised.extend(character.to_lowercase());
        }
        starting = false;
    }
    capitalised
}

/// Hash the function's one argument with `digest`, which answers the hexadecimal
/// form the Redshift hash functions report. The VALUE's own bytes are hashed, so
/// a binary value hashes to the digest of those bytes.
fn hashed<F>(ctx: &Context<'_>, digest: F) -> Result<Option<String>>
where
    F: Fn(&[u8]) -> String,
{
    Ok(raw_argument(ctx, 0).map(|bytes| digest(&bytes)))
}

/// One argument as the bytes it carries: a character string's own bytes, a
/// binary value's own bytes, and a number in the form the engine's implicit
/// conversion gives it. `None` for `NULL`.
fn raw_argument(ctx: &Context<'_>, index: usize) -> Option<Vec<u8>> {
    match ctx.get_raw(index) {
        ValueRef::Null => None,
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Some(bytes.to_vec()),
        ValueRef::Integer(value) => Some(value.to_string().into_bytes()),
        ValueRef::Real(value) => Some(value.to_string().into_bytes()),
    }
}

/// The current date and time, in the form the engine's own `CURRENT_TIMESTAMP`
/// reports: `YYYY-MM-DD HH:MM:SS`, so a value from either reads and compares the
/// same way.
fn timestamp_now() -> String {
    let now = ::aws_smithy_types::DateTime::from(SystemTime::now());
    let formatted = now
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    formatted
        .get(..19)
        .unwrap_or(&formatted)
        .replacen('T', " ", 1)
}

/// One argument as text, with the engine's implicit conversion of a number: a
/// Redshift function taking a character string accepts a number for it.
/// `None` for `NULL`, which makes every function here answer `NULL`.
fn text_argument(ctx: &Context<'_>, index: usize) -> Result<Option<String>> {
    match ctx.get_raw(index) {
        ValueRef::Null => Ok(None),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            Ok(Some(String::from_utf8_lossy(bytes).into_owned()))
        }
        ValueRef::Integer(value) => Ok(Some(value.to_string())),
        ValueRef::Real(value) => Ok(Some(value.to_string())),
    }
}

/// One argument as an integer, with the same implicit conversion the other way.
fn integer_argument(ctx: &Context<'_>, index: usize) -> Result<Option<i64>> {
    match ctx.get_raw(index) {
        ValueRef::Null => Ok(None),
        ValueRef::Integer(value) => Ok(Some(value)),
        #[allow(clippy::cast_possible_truncation)]
        ValueRef::Real(value) => Ok(Some(value as i64)),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            let text = String::from_utf8_lossy(bytes).into_owned();
            text.trim().parse::<i64>().map(Some).map_err(|_error| {
                failed(format!("invalid input syntax for integer: \"{text}\""))
            })
        }
    }
}

/// An argument kept as it is, for the functions that CHOOSE one of their
/// arguments rather than computing a new value.
fn owned(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(integer) => Value::Integer(integer),
        ValueRef::Real(real) => Value::Real(real),
        ValueRef::Text(bytes) => Value::Text(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(bytes) => Value::Blob(bytes.to_vec()),
    }
}

/// The engine error one of these functions reports: the function could not
/// produce a value for the arguments it was given.
fn failed(message: String) -> Error {
    Error::UserFunctionError(message.into())
}
