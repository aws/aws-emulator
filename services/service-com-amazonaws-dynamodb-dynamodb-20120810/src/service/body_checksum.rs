// SPDX-License-Identifier: CC0-1.0

//! The checksum every response of this service reports its own body under.
//!
//! DynamoDB stamps `x-amz-crc32` on every response it sends — a success exactly as a
//! failure — carrying the CRC-32 of the response body as the unsigned decimal integer a
//! client parses it as. A client that reads it (an AWS SDK validates every DynamoDB
//! response against this header) treats a body whose checksum does not match as corrupted
//! in transit and retries the call; a response that carries no checksum at all leaves that
//! integrity check with nothing to verify.
//!
//! It is a property of the RESPONSE rather than of any modelled shape: no output and no
//! error shape of this model declares it, so no handler can set it, and the framework's
//! `awsJson` renderers do not write it. That is why it is stamped at the layers every
//! response of this service passes through (`super::EmulatedService`) rather than by any
//! one operation.
//!
//! Nothing else about a response changes: the status, every other header and every byte of
//! the body stay as the renderer wrote them. A response that already reports a checksum
//! keeps the one it has, so two layers of one service cannot report two, and a header the
//! HTTP layer will not carry is dropped rather than turned into a transport failure that
//! would replace a modelled status.

use aws_smithy_runtime_api::http::Response as RtResponse;

/// The header a response reports the checksum of its own body in.
const CHECKSUM_HEADER: &str = "x-amz-crc32";

/// The response with the CRC-32 of its own body reported in [`CHECKSUM_HEADER`].
///
/// Idempotent and lossless: a response that already reports a checksum is returned
/// untouched, and a response whose body this layer cannot read (a streamed body) is left
/// alone — a checksum cannot be reported over bytes nothing here has seen.
pub fn with_body_checksum_header(mut resp: RtResponse) -> RtResponse {
    if resp.headers().get(CHECKSUM_HEADER).is_some() {
        return resp;
    }
    // A body built in memory by its renderer is readable here, which is every body this
    // service's renderers produce; the borrow of the body ends with this statement, so the
    // header can be written below.
    let Some(checksum) = resp.body().bytes().map(body_checksum) else {
        return resp;
    };
    let _ = resp.headers_mut().try_insert(CHECKSUM_HEADER, checksum);
    resp
}

/// The CRC-32 of `body`, rendered as the unsigned decimal integer the header carries.
fn body_checksum(body: &[u8]) -> String {
    let mut digest = crc32fast::Hasher::new();
    digest.update(body);
    digest.finalize().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::http::StatusCode;
    use aws_smithy_types::body::SdkBody;

    fn response(body: &'static str) -> RtResponse {
        RtResponse::new(
            StatusCode::try_from(200).expect("a valid status"),
            SdkBody::from(body),
        )
    }

    /// The value reported is the CRC-32 of the bytes the client receives, written as an
    /// unsigned decimal integer. The expectations are the PUBLISHED CRC-32 values of these
    /// strings (`0xCBF43926` is the standard check value of CRC-32 over `123456789`,
    /// `0xE8B7BE43` is its value over `a` and `0x352441C2` its value over `abc`), so this
    /// pins the algorithm and the rendering rather than comparing the code with itself.
    #[test]
    fn the_checksum_reported_is_the_crc32_of_the_body() {
        // (case, body, the CRC-32 of that body as an unsigned decimal integer)
        let rows: [(&str, &'static str, &str); 4] = [
            ("an empty body", "", "0"),
            ("the published check vector", "123456789", "3421780262"),
            ("one byte", "a", "3904355907"),
            ("three bytes", "abc", "891568578"),
        ];
        for (case, body, expected) in rows {
            let stamped = with_body_checksum_header(response(body));
            let reported = stamped
                .headers()
                .get(CHECKSUM_HEADER)
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the CRC-32 {expected}, got {reported:?}"
            );
        }
    }

    /// A response that already reports a checksum keeps the one it has: two layers of one
    /// service cannot report two.
    #[test]
    fn a_reported_checksum_is_left_as_it_was() {
        let once = with_body_checksum_header(response("abc"));
        let twice = with_body_checksum_header(once);
        let reported = twice.headers().get(CHECKSUM_HEADER).unwrap_or_default();
        assert_eq!(
            reported, "891568578",
            "case stamped twice: expected the one checksum of the body, got {reported:?}"
        );
    }
}
