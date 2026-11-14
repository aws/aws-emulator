// SPDX-License-Identifier: CC0-1.0

//! The ENVELOPE a received message is saved in when the `S3Action` that saves it names a KMS
//! key: the object the Amazon S3 encryption client reads back.
//!
//! `S3Action$KmsKeyArn` states what the object IS: "Your mail is encrypted by Amazon SES using
//! the Amazon S3 encryption client before the mail is submitted to Amazon S3 for storage. It is
//! not encrypted using Amazon S3 server-side encryption. This means that you must use the
//! Amazon S3 encryption client to decrypt the email after retrieving it from Amazon S3, as the
//! service has no access to use your Amazon Web Services KMS keys for decryption." So the
//! object's BODY is ciphertext and its user metadata is the envelope that client opens it from:
//! the data key sealed under the customer managed key ([`WRAPPED_KEY_METADATA`]), the
//! initialisation vector, the content-encryption algorithm, the key-wrapping algorithm, and the
//! material description the data key was sealed under ([`MATERIAL_DESCRIPTION_METADATA`]), which
//! for a received message names the key and the receipt rule that triggered the save.
//!
//! Nothing here holds key material of its own and nothing here is a second KMS: the data key is
//! generated and sealed by KMS ITSELF, over the emulator's own in-process transport, so the
//! wrapped key in the object's metadata is a ciphertext that KMS's own `Decrypt` opens — which
//! is what makes the stored object one a client really can decrypt, and the plaintext key is
//! used for this one message and dropped. The content encryption is the AEAD the envelope
//! declares, computed by the maintained cryptography crate this workspace seals with.

use ::ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ::ring::rand::{SecureRandom, SystemRandom};

/// The user-metadata entry carrying the data key, sealed under the customer managed key.
pub const WRAPPED_KEY_METADATA: &str = "x-amz-key-v2";

/// The user-metadata entry carrying the initialisation vector the content was encrypted under.
pub const IV_METADATA: &str = "x-amz-iv";

/// The user-metadata entry naming the content-encryption algorithm.
pub const CONTENT_ALGORITHM_METADATA: &str = "x-amz-cek-alg";

/// The user-metadata entry naming the algorithm the data key is wrapped with.
pub const WRAP_ALGORITHM_METADATA: &str = "x-amz-wrap-alg";

/// The user-metadata entry carrying the authentication tag's length in bits.
pub const TAG_LENGTH_METADATA: &str = "x-amz-tag-len";

/// The user-metadata entry carrying the material description — the encryption context the data
/// key is sealed under, which a reader has to present to unwrap it.
pub const MATERIAL_DESCRIPTION_METADATA: &str = "x-amz-matdesc";

/// The user-metadata entry carrying the length of the message before it was encrypted.
pub const PLAINTEXT_LENGTH_METADATA: &str = "x-amz-unencrypted-content-length";

/// The content-encryption algorithm the envelope declares: AES-256 in Galois/Counter Mode,
/// named as the Java cipher transformation the Amazon S3 encryption client writes.
pub const CONTENT_ALGORITHM: &str = "AES/GCM/NoPadding";

/// The key-wrapping algorithm the envelope declares: the data key is sealed by KMS.
pub const WRAP_ALGORITHM: &str = "kms";

/// The length in bits of the authentication tag AES-GCM appends.
pub const TAG_LENGTH_BITS: usize = 128;

/// The data key's length: AES-256, the key the content algorithm takes.
const DATA_KEY_SPEC: &str = "AES_256";

/// The material-description entry naming the KMS key the data key is sealed under, which is how
/// the Amazon S3 encryption client records it.
pub const MATERIAL_KEY_ID: &str = "kms_cmk_id";

/// One received message, encrypted: the object's body and the user metadata a reader opens it
/// from.
#[derive(Debug, Clone)]
pub struct Envelope {
    /// The ciphertext, with the authentication tag appended — the object's body.
    pub body: Vec<u8>,
    /// The object's user metadata, as `(name, value)` pairs.
    pub metadata: Vec<(String, String)>,
}

/// `plaintext` sealed for the key `kms_key_arn` names, with `material` as the material
/// description the data key is sealed under.
///
/// `None` when the envelope could not be made — KMS did not generate a data key under that key,
/// or the AEAD refused the message — in which case the caller saves nothing rather than saving a
/// message the action said to encrypt in the clear.
pub async fn sealed(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    kms_key_arn: &str,
    material: &[(String, String)],
    plaintext: &[u8],
) -> Option<Envelope> {
    let description = material_description(material);
    let generated = super::s3_action_authorization::kms_client(callback, region)
        .generate_data_key()
        .key_id(kms_key_arn)
        .key_spec(::aws_sdk_kms::types::DataKeySpec::from(DATA_KEY_SPEC))
        .set_encryption_context(Some(
            material
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        ))
        .send()
        .await;
    let generated = match generated {
        Ok(generated) => generated,
        Err(failure) => {
            ::tracing::warn!(
                kms_key = ?kms_key_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
                "no data key could be generated under the receipt rule's encryption key, so the \
                 received message is not saved"
            );
            return None;
        }
    };
    let data_key = generated.plaintext?;
    let wrapped = generated.ciphertext_blob?;
    let cipher = UnboundKey::new(&AES_256_GCM, data_key.as_ref())
        .ok()
        .map(LessSafeKey::new)?;
    let mut iv = [0u8; NONCE_LEN];
    SystemRandom::new().fill(&mut iv).ok()?;
    // `seal_in_place_append_tag` leaves the buffer holding the ciphertext followed by the
    // authentication tag, which is the body the Amazon S3 encryption client reads.
    let mut body = plaintext.to_vec();
    cipher
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(iv),
            Aad::empty(),
            &mut body,
        )
        .ok()?;
    Some(Envelope {
        body,
        metadata: vec![
            (
                WRAPPED_KEY_METADATA.to_string(),
                ::aws_smithy_types::base64::encode(wrapped.as_ref()),
            ),
            (
                IV_METADATA.to_string(),
                ::aws_smithy_types::base64::encode(iv.as_slice()),
            ),
            (
                CONTENT_ALGORITHM_METADATA.to_string(),
                CONTENT_ALGORITHM.to_string(),
            ),
            (WRAP_ALGORITHM_METADATA.to_string(), WRAP_ALGORITHM.to_string()),
            (TAG_LENGTH_METADATA.to_string(), TAG_LENGTH_BITS.to_string()),
            (MATERIAL_DESCRIPTION_METADATA.to_string(), description),
            (
                PLAINTEXT_LENGTH_METADATA.to_string(),
                plaintext.len().to_string(),
            ),
        ],
    })
}

/// `material` as the material description the object carries: the JSON object of the
/// key-value pairs the data key is sealed under.
fn material_description(material: &[(String, String)]) -> String {
    let described: ::serde_json::Map<String, ::serde_json::Value> = material
        .iter()
        .map(|(name, value)| (name.clone(), ::serde_json::Value::String(value.clone())))
        .collect();
    ::serde_json::Value::Object(described).to_string()
}
