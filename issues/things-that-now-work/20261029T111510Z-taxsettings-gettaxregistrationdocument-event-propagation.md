Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 74ed32df9318f7a3c6c2e67cb9b84328e4fd5823

# GetTaxRegistrationDocument delivers no object: the documented download to the caller's S3 bucket never happens and the submitted document bytes are never stored

_Filed: 2026-10-29T11:15:10Z_
_Operation: GetTaxRegistrationDocument | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `74ed32df9318f7a3c6c2e67cb9b84328e4fd5823`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states what this operation DOES, not merely what it answers: "Downloads your tax documents to the Amazon S3 bucket that you specify in your request." The handler performs no download. It resolves the submitted `taxDocumentMetadata` against stored metadata, formats a `destinationFilePath` string and formats a `presignedS3Url` string, and returns. No object is written anywhere — not into the caller's bucket (`destinationS3Location.bucket`), and not into the service's own `aws-tax-documents-<accountId>` bucket that the URL it reports names. This repository ships an implemented S3 emulator whose state other service crates in this repo already write objects into (firehose, gamelift, lightsail, bcmdataexports), so the delivery mechanism exists and is used; this crate has no dependency on it at all.

The root cause is upstream in the same crate: the write path keeps only a document's NAME. `crate::state::document_metadatas` derives a `TaxDocumentMetadata { tax_document_access_token, tax_document_name }` per submitted document and discards everything else, so `TaxRegistrationDocFile$fileContent` (a `@required` blob of 1..5242880 bytes) and `SourceS3Location { bucket, key }` are read only to pick a name. `state.rs` admits this in prose. With no bytes held, there is nothing for a download to deliver, so every `GetTaxRegistrationDocument` 200 reports a location at which no object exists: a client that follows the reported `presignedS3Url` against the S3 emulator, or that lists the bucket it named in `destinationS3Location`, finds nothing, even though the call reported success.

This is NOT the already-open `parameter-ignored` issue on this operation ("GetTaxRegistrationDocument drops destinationS3Location.bucket, so the response is identical whatever bucket the caller names"). That issue's defect and fix are about the RESPONSE: the required `bucket` member not influencing `destinationFilePath`. A fixer can satisfy it entirely by threading `bucket` into the reported path while still delivering no object and still storing no document bytes. The defect filed here is the cross-service delivery itself (and the content the write drops, which makes delivery impossible), which no existing issue asks for.

## Evidence

Model — the operation's own documented ACTION (`smithy_query(query="doc", shape="com.amazonaws.taxsettings#GetTaxRegistrationDocument")`):
> "Downloads your tax documents to the Amazon S3 bucket that you specify in your request."

Model — `smithy_query(query="shape", shape="TaxRegistrationDocFile")`:
> "fileContent": {"target": "FileBlob", "required": true, "length": {"min": 1, "max": 5242880}, "type": "blob", "doc": "<p>The tax registration document content. </p>"}

Handler — the whole body does one lookup and two string formats, with no write of any kind, `services/service-com-amazonaws-taxsettings-taxsettings/src/ops/get_tax_registration_document.rs:172-185`:
```
        let document = {
            let store = state.lock();
            store
                .held_tax_document(&input.tax_document_metadata)
                .map_err(GetTaxRegistrationDocumentErrorModeled::ValidationException)?
        };
        Ok(GetTaxRegistrationDocumentOutput {
            destination_file_path: input
                .destination_s3_location
                .as_ref()
                .map(|destination| destination_file_path(destination, &document.name)),
            presigned_s3_url: Some(crate::state::document_download_url(&document)),
        })
```

The same file's header asserts the reported path is "the file path the request asked it to be downloaded to" — a download the code above never performs, `src/ops/get_tax_registration_document.rs:13-17`:
```
//! * `destinationFilePath` — reported only when the request names a
//!   `destinationS3Location`: the object key the document occupies in THAT
//!   bucket, which is the file path the request asked it to be downloaded to. A
//!   request that names no destination asked for no download, so there is no
//!   destination path to report.
```

The crate admits the bytes are not kept, `src/state.rs` (doc of `document_download_url`, lines 1589-1593):
```
/// It carries no query signature: the model caps `Url` at 200 characters, which
/// no SigV4 query-signed form fits (its signature parameters alone are longer
/// than that), and this emulator keeps each document's metadata rather than its
/// content, so there is no object body for a signature to authorise.
```
and the write that drops them, `src/state.rs:1449-1466` (`document_metadatas`):
```
            let name = document
                .file
                .as_ref()
                .map(|file| file.file_name.clone())
                .or_else(|| document.s3_location.as_ref().map(|loc| loc.key.clone()))
                .unwrap_or_else(|| format!("document-{}", index + 1));
            TaxDocumentMetadata {
                tax_document_access_token: document_access_token(
                    account_id, revision, index, &name,
                ),
                tax_document_name: name,
            }
```
`StoredTaxRegistration.tax_document_metadatas: Vec<TaxDocumentMetadata>` (`src/state.rs:1141`) is the only place a submitted document survives, and that shape carries a token and a name only.

The mechanism exists in this repo and is used from a service's own `src/`: `services/service-com-amazonaws-firehose-firehose-20150804/src/state/delivery.rs:1733-1747`:
```
    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
...
        let stored = ::service_com_amazonaws_s3_amazons3::state::Object {
```
and the S3 write it targets is implemented: `services/service-com-amazonaws-s3-amazons3/src/ops/put_object.rs:12` — "//! Implemented here — this is the ONLY file to touch for this operation." (no `//! SCAFFOLDED, NOT IMPLEMENTED:` header). This crate reaches no other service: `services/service-com-amazonaws-taxsettings-taxsettings/Cargo.toml:9-23` lists only `engine`, the smithy/runtime crates, `tokio`, `bytes`, `http`, `serde_json`, `tracing` — no `service-com-amazonaws-s3-amazons3`.

Test coverage: `tests/all/tax_registration_document.rs` exercises the operation over every destination shape (`:217-228` — no destination, a bucket alone, a bucket with a prefix, a prefix with a trailing separator) and pins the reported URL form (`:271-287`) and path (`:312-319`), but no test mounts the S3 emulator or asserts that any object exists after the call, so nothing pins the absence of delivery with a stated reason. A fix that writes the object can leave the pinned URL/path strings exactly as they are.

## Steps to Reproduce

1. POST /PutTaxRegistration with {"accountId":"000000000001","taxRegistrationEntry":{"registrationId":"DE136695976","registrationType":"VAT","legalName":"Acme GmbH","legalAddress":{"postalCode":"10115","countryCode":"DE"},"verificationDetails":{"taxRegistrationDocuments":[{"file":{"fileName":"trn.pdf","fileContent":"aGVsbG8="}}]}}} and observe 200.
2. POST /GetTaxRegistration with {"accountId":"000000000001"} and copy taxRegistration.taxDocumentMetadatas[0].
3. Mount the S3 emulator on the same engine and create the bucket you intend to download into: PUT /my-tax-documents (CreateBucket).
4. POST /GetTaxRegistrationDocument with {"taxDocumentMetadata":<that metadata>,"destinationS3Location":{"bucket":"my-tax-documents","prefix":"tax/2026"}} and observe 200 with destinationFilePath "tax/2026/trn.pdf" and presignedS3Url "https://aws-tax-documents-000000000001.s3.amazonaws.com/1/0".
5. GET /my-tax-documents?list-type=2 against the S3 emulator: the bucket is empty. The operation documented as "Downloads your tax documents to the Amazon S3 bucket that you specify in your request" delivered nothing to the bucket it reported a key in.
6. GET the object the response's own presignedS3Url names (bucket aws-tax-documents-000000000001, key 1/0): no such bucket and no such object exists either, so the reported download location resolves to nothing.
7. Grep the crate for any reference to another service's state: `grep -rn 'service_com_amazonaws_s3_amazons3' services/service-com-amazonaws-taxsettings-taxsettings/` returns nothing, and `grep -rn 'file_content' services/service-com-amazonaws-taxsettings-taxsettings/src/` shows the submitted bytes are never read.
