// SPDX-License-Identifier: CC0-1.0

//! The WRITE this service makes into a caller's object store, and what it establishes.
//!
//! Two kinds of job of this service leave a work product behind at the `s3OutputPath` the
//! start configured — a patient insights job ([`super::insights_run`]) and a medical coding
//! job ([`super::medical_coding_run`]) — and both make that write the same way: through the
//! in-process Amazon S3 client this workspace builds once
//! ([`service_common::s3_client`]), so the objects are held by the service that OWNS them
//! and a client reads them back through that service's own API.
//!
//! What a write ESTABLISHES is stated here once as well, because both callers settle their
//! job's status on it: a store that ANSWERS a refusal is a job that could not store its
//! work product, while a call NOTHING answered — no object store is mounted in this
//! assembly — establishes nothing at all, so the caller leaves the write owed rather than
//! recording a failure nobody reported.

use smithy_server_runtime::EmbeddedEmulator;

/// The media type the output documents are stored with: they are JSON documents, and a
/// client reads one straight off the location the job reports.
const OUTPUT_CONTENT_TYPE: &str = "application/json";

/// The integrity checksum the write of an output document DECLARES, which is the checksum
/// the object store records for the object and reports back on a read that enables the
/// checksum mode (`GetObjectOutput$ChecksumCRC32`, under the `FULL_OBJECT` type a one-shot
/// write stores).
///
/// A service writes into a caller's bucket through an AWS SDK, and an SDK computes a CRC32
/// checksum for every upload it makes unless the caller asks for another algorithm (the
/// default request checksum calculation of every current SDK), so the objects a finished job
/// leaves behind carry that checksum and a client that downloads one can verify the bytes it
/// got. Named here rather than left to the transport: the in-process hop every emulated
/// service shares computes a checksum only where one is asked for
/// (`service_common::s3_client`), so this is the SERVICE stating what its write carries.
const OUTPUT_CHECKSUM_ALGORITHM: ::aws_sdk_s3::types::ChecksumAlgorithm =
    ::aws_sdk_s3::types::ChecksumAlgorithm::Crc32;

/// Write `objects` — each one a key and the document it carries — into `bucket` in
/// `region`, and report what the object store answered.
///
/// `Some(true)` when the store took every object, `Some(false)` when it ANSWERED a refusal
/// of one — the bucket does not exist, or the write is not permitted — which is a job that
/// could not store its work product. `None` when nothing answered the write at all: no
/// object store is mounted in this assembly, so nothing is established and the caller keeps
/// the transition the clock owed it.
pub(super) async fn write_objects(
    callback: &EmbeddedEmulator,
    region: &str,
    bucket: &str,
    objects: &[(String, Vec<u8>)],
) -> Option<bool> {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let client = ::service_common::s3_client::s3_client(callback, region);
    for (key, document) in objects {
        let written = client
            .put_object()
            .bucket(bucket)
            .key(key)
            .content_type(OUTPUT_CONTENT_TYPE)
            .checksum_algorithm(OUTPUT_CHECKSUM_ALGORITHM)
            .body(::aws_sdk_s3::primitives::ByteStream::from(document.clone()))
            .send()
            .await;
        match written {
            Ok(_) => {}
            Err(error) => match error.code() {
                // The store ANSWERED: this job's output could not be stored where it was
                // configured to go.
                Some(code) => {
                    ::tracing::warn!(
                        bucket = %bucket,
                        code = %code,
                        "a job's output was refused by the emulator's Amazon S3"
                    );
                    return Some(false);
                }
                // Nothing answered the write — the same reading
                // `service_common::s3_object_read` gives a read nothing answered.
                None => {
                    ::tracing::warn!(
                        bucket = %bucket,
                        "nothing answered a job's output write: this emulator has no Amazon S3 \
                         to store it in"
                    );
                    return None;
                }
            },
        }
    }
    Some(true)
}
