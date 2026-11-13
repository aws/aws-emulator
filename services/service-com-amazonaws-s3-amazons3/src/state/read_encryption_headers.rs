// SPDX-License-Identifier: CC0-1.0

//! The SERVER-SIDE ENCRYPTION headers a READ of an object may not carry, and the refusal one
//! that does earns.
//!
//! The encryption an object is stored under is chosen by the WRITE that stored it: `PutObject`,
//! the copies and `CreateMultipartUpload` bind `x-amz-server-side-encryption` and the three
//! headers that qualify it (`smithy_query(query="operation",operation="PutObject")` reports
//! `ServerSideEncryption`, `SSEKMSKeyId`, `SSEKMSEncryptionContext` and `BucketKeyEnabled` as
//! `@httpHeader` members of its input). A READ — `GetObject`, `HeadObject`,
//! `GetObjectAttributes` — binds NONE of them: the object's configuration is what those
//! operations REPORT in headers of the same names, and the only encryption members their input
//! carries are the SSE-C three, which name the customer-provided key the stored bytes come of
//! ([`super::sse_customer`]).
//!
//! So a read that carries one of them asks the service to encrypt something it is not writing.
//! S3 refuses it for the HEADER — `InvalidArgument` (400), stating that the header is not
//! supported for this operation and naming it and the value as sent in the `<ArgumentName>` /
//! `<ArgumentValue>` pair its document carries — rather than serving the plaintext and
//! ignoring what the caller asked for.
//!
//! ONE catalog and one reading, so every read of this service refuses the same header alike.

/// The `x-amz-server-side-encryption` header a WRITE names the encryption to apply in, which a
/// read does not bind.
const SERVER_SIDE_ENCRYPTION_HEADER: &str = "x-amz-server-side-encryption";

/// Every header a write binds the S3-MANAGED encryption of the object it stores from, in the
/// order S3's own documentation lists them: the algorithm, the KMS key it names, the encryption
/// context that key is used with, and whether the bucket key is enabled for the object.
///
/// Source: the `@httpHeader` members of `PutObject`'s input
/// (`smithy_query(query="operation",operation="PutObject")` reports
/// `ServerSideEncryption: {httpHeader: "x-amz-server-side-encryption"}`,
/// `SSEKMSKeyId: {httpHeader: "x-amz-server-side-encryption-aws-kms-key-id"}`,
/// `SSEKMSEncryptionContext: {httpHeader: "x-amz-server-side-encryption-context"}` and
/// `BucketKeyEnabled: {httpHeader: "x-amz-server-side-encryption-bucket-key-enabled"}`), none
/// of which a read's input binds.
const WRITE_ONLY_ENCRYPTION_HEADERS: [&str; 4] = [
    SERVER_SIDE_ENCRYPTION_HEADER,
    "x-amz-server-side-encryption-aws-kms-key-id",
    "x-amz-server-side-encryption-context",
    "x-amz-server-side-encryption-bucket-key-enabled",
];

/// The write-only encryption header `parts` carries and the value it carries it with, or `None`
/// for a request carrying none of them.
///
/// The FIRST one in the catalog's order is what answers, the way S3's document reports one
/// argument; a header present with an empty value states no encryption and is not one.
pub fn unsupported_read_encryption_header(
    parts: &::http::request::Parts,
) -> Option<(&'static str, String)> {
    WRITE_ONLY_ENCRYPTION_HEADERS.iter().find_map(|header| {
        parts
            .headers
            .get(*header)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| (*header, value.to_string()))
    })
}

/// The `<Message>` S3 answers such a header with.
///
/// The sentence is fixed for the class and names the OFFENDING HEADER, which is the one thing
/// about it that varies: the caller has to drop that header, and the pair beside the sentence
/// carries the value it was sent with.
pub fn unsupported_read_encryption_message(header: &str) -> String {
    format!("{header} header is not supported for this operation.")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request carrying `headers`, as a read of an object arrives.
    fn parts(headers: &[(&str, &str)]) -> ::http::request::Parts {
        let mut builder = ::http::Request::builder()
            .method("GET")
            .uri("https://s3.us-east-1.amazonaws.com/bucket/key");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder
            .body(())
            .expect("the request must build")
            .into_parts()
            .0
    }

    /// One row of the header table: the case, the headers the read carried, and the header the
    /// refusal must name (`None` for a read that carries none to refuse).
    type HeaderRow<'a> = (&'a str, &'a [(&'a str, &'a str)], Option<&'a str>);

    /// Every write-only encryption header is refused on a read, whatever it carries, and the
    /// SSE-C members a read does bind are not.
    #[test]
    fn a_read_may_carry_no_write_only_encryption_header() {
        // (case, the headers the read carried, the header the refusal must name)
        let rows: [HeaderRow<'_>; 8] = [
            ("no encryption header at all", &[], None),
            (
                "the customer-provided key members a read does bind",
                &[
                    ("x-amz-server-side-encryption-customer-algorithm", "AES256"),
                    ("x-amz-server-side-encryption-customer-key-MD5", "digest"),
                ],
                None,
            ),
            (
                "the algorithm",
                &[("x-amz-server-side-encryption", "AES256")],
                Some("x-amz-server-side-encryption"),
            ),
            (
                "the KMS algorithm",
                &[("x-amz-server-side-encryption", "aws:kms")],
                Some("x-amz-server-side-encryption"),
            ),
            (
                "a KMS key id",
                &[(
                    "x-amz-server-side-encryption-aws-kms-key-id",
                    "arn:aws:kms:us-east-1:123456789012:key/abc",
                )],
                Some("x-amz-server-side-encryption-aws-kms-key-id"),
            ),
            (
                "an encryption context",
                &[("x-amz-server-side-encryption-context", "e30=")],
                Some("x-amz-server-side-encryption-context"),
            ),
            (
                "the bucket key flag",
                &[(
                    "x-amz-server-side-encryption-bucket-key-enabled",
                    "true",
                )],
                Some("x-amz-server-side-encryption-bucket-key-enabled"),
            ),
            (
                "a header present with no value states no encryption",
                &[("x-amz-server-side-encryption", "")],
                None,
            ),
        ];
        for (case, headers, expected) in rows {
            let refused = unsupported_read_encryption_header(&parts(headers));
            assert_eq!(
                refused.as_ref().map(|(header, _)| *header),
                expected,
                "case {case:?}: expected {expected:?}, got {refused:?}"
            );
            if let Some((header, value)) = refused {
                assert_eq!(
                    header, expected.unwrap_or_default(),
                    "case {case:?}: the refusal names the header it refused"
                );
                assert!(
                    !value.is_empty(),
                    "case {case:?}: the pair echoes the value as sent, got {value:?}"
                );
            }
        }
    }
}
