// SPDX-License-Identifier: CC0-1.0

//! What a response that carries only PART of an object advertises about its checksum.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour a checksum-validating client sees: the `x-amz-checksum-*` header beside
//! the body it arrived with.
//!
//! The rule under test, from the members that carry the value: every `Checksum*` member is
//! "the Base64 encoded … checksum of the object" (each member's own documentation), and
//! `GetObject` carries `@httpChecksum{requestValidationModeMember: "ChecksumMode"}`, so a
//! client with response-checksum validation on RECOMPUTES the digest over the body it
//! received and compares. The value a response advertises therefore has to be the digest of
//! the bytes that response carries:
//!
//! * a read of the whole object advertises the checksum the write recorded;
//! * a read narrowed to exactly one part of the object advertises THAT part's own digest —
//!   "effectively performs a 'ranged' GET request for the part specified"
//!   (`GetObjectRequest$PartNumber`), and the part's digest is what the body then is;
//! * any other narrowed read advertises NO checksum at all, because no digest the store
//!   holds describes those bytes;
//! * a `HeadObject` of the same span advertises exactly what the `GetObject` does — "a HEAD
//!   request has the same options as a GET operation on an object" — and an object written
//!   under no checksum algorithm of its own advertises the default S3 added to it ("if it was
//!   uploaded without a checksum (and Amazon S3 added the default checksum, `CRC64NVME`, to
//!   the uploaded object)", `PutObjectOutput$ChecksumCRC64NVME`) under exactly the same rule.
//!
//! The rows differ along what the rule generalises over: how the span is NAMED (a part
//! number, a closed byte range, an open-ended one, a suffix one, none at all), whether that
//! span is the whole object, exactly one part of it or neither, how the object was WRITTEN
//! (a one-shot write, a multipart upload of two parts), and whether it was written under a
//! checksum algorithm at all.
//!
//! The expected digests are computed here, from the bytes each row expects in the body,
//! rather than read back from the crate under test: `sha2` for the digest, the AWS SDK's own
//! checksum crate for the default algorithm, the Base64 of the
//! AWS types crate for the encoding, and the composition the Amazon S3 User Guide publishes
//! for a multipart object's COMPOSITE value ("concatenates the … checksums … and then
//! calculates the checksum of these concatenated values", reported "with the total number of
//! parts" after a dash).

use aws_smithy_types::base64;
use sha2::{Digest, Sha256};

use super::bucket_lifecycle::{router, send, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_partial_reads::{completed_multipart_object, seeded_payload};

/// The `ChecksumAlgorithm` value every row in this suite writes under, and the response
/// header its value travels on (`ChecksumSHA256 -> x-amz-checksum-sha256`).
const ALGORITHM: &str = "SHA256";
/// See [`ALGORITHM`].
const ALGORITHM_HEADER: &str = "x-amz-checksum-sha256";

/// The algorithm an object written under none of its own carries — "Amazon S3 added the
/// default checksum, `CRC64NVME`, to the uploaded object"
/// (`PutObjectOutput$ChecksumCRC64NVME`) — and the header its value travels on.
const DEFAULT_ALGORITHM: &str = "CRC64NVME";
/// See [`DEFAULT_ALGORITHM`].
const DEFAULT_ALGORITHM_HEADER: &str = "x-amz-checksum-crc64nvme";

/// Every `x-amz-checksum-*` response header the model binds, so a row that expects NO
/// checksum can assert that none of them is there rather than only the one it wrote under.
const EVERY_CHECKSUM_HEADER: [&str; 11] = [
    "x-amz-checksum-crc32",
    "x-amz-checksum-crc32c",
    "x-amz-checksum-crc64nvme",
    "x-amz-checksum-sha1",
    "x-amz-checksum-sha256",
    "x-amz-checksum-sha512",
    "x-amz-checksum-md5",
    "x-amz-checksum-xxhash64",
    "x-amz-checksum-xxhash3",
    "x-amz-checksum-xxhash128",
    "x-amz-checksum-type",
];

/// The fewest bytes a part other than the last one may carry: "Part size: 5 MiB to 5 GiB.
/// There is no minimum size limit on the last part of your multipart upload" (Amazon S3 User
/// Guide, *Amazon S3 multipart upload limits*).
const MIN_MULTIPART_PART_BYTES: usize = 5 * 1024 * 1024;

/// The Base64 SHA-256 digest of `data`, as a `Checksum*` member carries it.
fn digest_of(data: &[u8]) -> String {
    base64::encode(Sha256::digest(data))
}

/// The Base64 digest of `data` under [`DEFAULT_ALGORITHM`], computed by the AWS SDK's own
/// implementation of that checksum rather than by the crate under test.
fn default_digest_of(data: &[u8]) -> String {
    let parsed: aws_smithy_checksums::ChecksumAlgorithm = DEFAULT_ALGORITHM
        .to_ascii_lowercase()
        .parse()
        .expect("the default algorithm is one this crate computes");
    let mut checksum = parsed.into_impl();
    checksum.update(data);
    base64::encode(checksum.finalize())
}

/// The COMPOSITE value a multipart object assembled from `parts` carries: the digest of the
/// parts' RAW digests, Base64 encoded, with the part count after a dash.
fn composite_of(parts: &[Vec<u8>]) -> String {
    let mut digests: Vec<u8> = Vec::new();
    for part in parts {
        digests.extend_from_slice(&Sha256::digest(part));
    }
    format!("{}-{}", base64::encode(Sha256::digest(&digests)), parts.len())
}

/// What a row expects the response to advertise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Advertised {
    /// The value the write recorded for the WHOLE object — what a read of all of it answers.
    WholeObject,
    /// The digest of exactly the bytes the response carries.
    OfTheBody,
    /// The DEFAULT algorithm's digest of the whole object — what a read of all of an object
    /// written under no algorithm of its own answers.
    DefaultWholeObject,
    /// No `x-amz-checksum-*` header at all.
    Nothing,
}

/// One row: the case name, which stored object it reads, the span it names and what the
/// response must advertise.
struct Row {
    case: &'static str,
    key: &'static str,
    /// The `partNumber` query parameter, when the row names one.
    part_number: Option<i32>,
    /// The `Range` header, when the row names one.
    range: Option<&'static str>,
    advertised: Advertised,
}

#[tokio::test]
async fn a_response_advertises_the_checksum_of_the_bytes_it_carries() {
    let router = router();
    let name = "span-checksum-bucket";
    bucket(&router, name).await;

    // A multipart object, so a part names a span that is NOT the whole object; a one-shot
    // write, where the object has a single part — itself; and one written under no checksum
    // algorithm at all.
    let parts = vec![
        seeded_payload(0x1234_5678, MIN_MULTIPART_PART_BYTES),
        seeded_payload(0x9abc_def0, 1_024),
    ];
    completed_multipart_object(
        &router,
        name,
        "assembled.bin",
        &parts,
        &[("x-amz-checksum-algorithm", ALGORITHM)],
    )
    .await;
    let whole = seeded_payload(0x0f0f_0f0f, 4_096);
    for (key, headers) in [
        (
            "written.bin",
            vec![("x-amz-sdk-checksum-algorithm", ALGORITHM)],
        ),
        ("unchecksummed.bin", Vec::new()),
    ] {
        let stored = put_object(&router, name, key, &whole, &headers).await;
        assert_eq!(
            stored.status,
            200,
            "the write of {key:?} must succeed, got {} with body {:?}",
            stored.status,
            text(&stored)
        );
    }

    // The whole-object values each stored object carries, computed here from its bytes.
    let composite = composite_of(&parts);
    let one_shot = digest_of(&whole);

    let rows: Vec<Row> = vec![
        Row {
            case: "the whole multipart object",
            key: "assembled.bin",
            part_number: None,
            range: None,
            advertised: Advertised::WholeObject,
        },
        Row {
            case: "a range covering every byte of the multipart object",
            key: "assembled.bin",
            part_number: None,
            range: Some("bytes=0-"),
            advertised: Advertised::WholeObject,
        },
        Row {
            case: "the first part by part number",
            key: "assembled.bin",
            part_number: Some(1),
            range: None,
            advertised: Advertised::OfTheBody,
        },
        Row {
            case: "the last part by part number",
            key: "assembled.bin",
            part_number: Some(2),
            range: None,
            advertised: Advertised::OfTheBody,
        },
        Row {
            case: "the first part named as a closed byte range",
            key: "assembled.bin",
            part_number: None,
            range: Some("bytes=0-5242879"),
            advertised: Advertised::OfTheBody,
        },
        Row {
            case: "the last part named as an open-ended byte range",
            key: "assembled.bin",
            part_number: None,
            range: Some("bytes=5242880-"),
            advertised: Advertised::OfTheBody,
        },
        Row {
            case: "a closed range inside the first part",
            key: "assembled.bin",
            part_number: None,
            range: Some("bytes=10-99"),
            advertised: Advertised::Nothing,
        },
        Row {
            case: "an open-ended range starting inside the first part",
            key: "assembled.bin",
            part_number: None,
            range: Some("bytes=10-"),
            advertised: Advertised::Nothing,
        },
        Row {
            case: "a suffix range of the multipart object",
            key: "assembled.bin",
            part_number: None,
            range: Some("bytes=-100"),
            advertised: Advertised::Nothing,
        },
        Row {
            case: "a range spanning both parts but not all of the first",
            key: "assembled.bin",
            part_number: None,
            range: Some("bytes=1024-5243000"),
            advertised: Advertised::Nothing,
        },
        Row {
            case: "a range unit this service does not understand, which serves it whole",
            key: "assembled.bin",
            part_number: None,
            range: Some("items=0-9"),
            advertised: Advertised::WholeObject,
        },
        Row {
            case: "the whole one-shot object",
            key: "written.bin",
            part_number: None,
            range: None,
            advertised: Advertised::WholeObject,
        },
        Row {
            case: "part 1 of a one-shot write, which is the whole object",
            key: "written.bin",
            part_number: Some(1),
            range: None,
            advertised: Advertised::WholeObject,
        },
        Row {
            case: "an open-ended range of the one-shot object",
            key: "written.bin",
            part_number: None,
            range: Some("bytes=10-"),
            advertised: Advertised::Nothing,
        },
        Row {
            case: "a one-byte range of the one-shot object",
            key: "written.bin",
            part_number: None,
            range: Some("bytes=2048-2048"),
            advertised: Advertised::Nothing,
        },
        Row {
            case: "a suffix range of the one-shot object",
            key: "written.bin",
            part_number: None,
            range: Some("bytes=-16"),
            advertised: Advertised::Nothing,
        },
        Row {
            case: "the whole of an object written under no checksum algorithm of its own",
            key: "unchecksummed.bin",
            part_number: None,
            range: None,
            advertised: Advertised::DefaultWholeObject,
        },
        Row {
            case: "a range of an object written under no checksum algorithm of its own",
            key: "unchecksummed.bin",
            part_number: None,
            range: Some("bytes=10-"),
            advertised: Advertised::Nothing,
        },
    ];

    for row in &rows {
        let case = row.case;
        let query = row
            .part_number
            .map(|number| format!("?partNumber={number}"))
            .unwrap_or_default();
        let mut headers: Vec<(&str, &str)> = vec![("x-amz-checksum-mode", "ENABLED")];
        if let Some(range) = row.range {
            headers.push(("range", range));
        }
        let answer = get_object(&router, name, row.key, &query, &headers).await;
        assert!(
            answer.status == 200 || answer.status == 206,
            "case {case:?}: the read must be served, got {} with body {:?}",
            answer.status,
            text(&answer)
        );

        // What the whole object's value is, for the rows that expect it.
        let stored = match row.key {
            "assembled.bin" => Some(composite.clone()),
            "written.bin" => Some(one_shot.clone()),
            _ => None,
        };
        // The header the value travels on, and the value itself.
        let expected: Option<(&str, String)> = match row.advertised {
            Advertised::WholeObject => stored.map(|value| (ALGORITHM_HEADER, value)),
            Advertised::OfTheBody => Some((ALGORITHM_HEADER, digest_of(&answer.body))),
            Advertised::DefaultWholeObject => {
                Some((DEFAULT_ALGORITHM_HEADER, default_digest_of(&whole)))
            }
            Advertised::Nothing => None,
        };

        let carries = match &expected {
            Some((header, value)) => {
                assert_eq!(
                    answer.header(header),
                    Some(value.clone()),
                    "case {case:?}: expected the checksum {value:?} on {header}, got {:?}",
                    answer.header(header)
                );
                // A value that describes the body it arrived with: every row but the
                // whole-object COMPOSITE one (whose value is a composition over the parts
                // rather than a digest of the bytes, which is what its `-<parts>` suffix
                // says) must equal the digest of the body.
                if !value.contains('-') {
                    let of_the_body = match *header {
                        DEFAULT_ALGORITHM_HEADER => default_digest_of(&answer.body),
                        _ => digest_of(&answer.body),
                    };
                    assert_eq!(
                        *value,
                        of_the_body,
                        "case {case:?}: the advertised checksum must be the digest of the \
                         {} bytes in the body, got {value:?}",
                        answer.body.len()
                    );
                }
                Some(*header)
            }
            None => None,
        };
        // Whatever one algorithm a response advertises, it advertises no other: a body
        // carries the digest of ONE algorithm, and a span no stored checksum describes
        // carries none at all.
        for header in EVERY_CHECKSUM_HEADER {
            if carries == Some(header) || (carries.is_some() && header == "x-amz-checksum-type")
            {
                continue;
            }
            assert_eq!(
                answer.header(header),
                None,
                "case {case:?}: a response carrying bytes no stored checksum describes must \
                 advertise none, got {header} = {:?}",
                answer.header(header)
            );
        }

        // A HEAD of the same span advertises exactly what the GET did.
        let headed = head_of(&router, name, row.key, &query, &headers).await;
        for header in EVERY_CHECKSUM_HEADER {
            assert_eq!(
                headed.header(header),
                answer.header(header),
                "case {case:?}: a HEAD of the same span must advertise what the GET did on \
                 {header} ({:?}), got {:?}",
                answer.header(header),
                headed.header(header)
            );
        }
    }

    // And the span itself is unaffected by any of this: the open-ended range the rule is
    // most often exercised through still answers 206 with the bytes it names.
    let ranged = get_object(
        &router,
        name,
        "written.bin",
        "",
        &[("x-amz-checksum-mode", "ENABLED"), ("range", "bytes=10-")],
    )
    .await;
    assert_eq!(
        ranged.status, 206,
        "case \"an open-ended range\": expected the partial-content status, got {}",
        ranged.status
    );
    assert_eq!(
        ranged.header("content-range"),
        Some(format!("bytes 10-{}/{}", whole.len() - 1, whole.len())),
        "case \"an open-ended range\": expected the span of the whole object, got {:?}",
        ranged.header("content-range")
    );
    assert_eq!(
        ranged.body.as_ref(),
        &whole[10..],
        "case \"an open-ended range\": expected the span's own bytes, got {} of them",
        ranged.body.len()
    );
}

/// `HEAD /<bucket>/<key><query>` — `HeadObject`, with the headers the row carries.
async fn head_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(router, "HEAD", &format!("/{bucket}/{key}{query}"), headers, "").await
}
