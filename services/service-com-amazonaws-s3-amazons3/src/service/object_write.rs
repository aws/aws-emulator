// SPDX-License-Identifier: CC0-1.0

//! What an object WRITE must satisfy about the name and the class it names, before anything
//! is stored: the bytes an object key may carry, the storage classes the bucket it is
//! written into can host, and the query parameters that bucket's endpoint serves at all.
//!
//! Every rule here is the bucket's and the key's rather than one operation's, so every write
//! that names a key or a class — `PutObject`, `CopyObject`, `CreateMultipartUpload` — is
//! judged by the same functions here and answers the same code for the same value.

use crate::state::{self, Bucket};

/// The code S3 answers a key outside the published byte rules with, and the status it carries
/// (Amazon S3 "Error responses", list of error codes: "KeyTooLongError — Your key is too
/// long", HTTP 400).
pub const KEY_TOO_LONG_CODE: &str = "KeyTooLongError";

/// The code naming the refusal that carries NO error document at all: HTTP 400 with an empty
/// body, which is what S3 answers a request whose URI could not be read as a request in the
/// first place — the rejection happens before the S3 API layer, so there is no `<Code>` for a
/// client to branch on and an SDK reports the STATUS as the code it saw.
///
/// A key carrying a NUL (0x00) byte is that request: no URI can convey the byte, so there is
/// no addressable object for the API to refuse. [`crate::service::wire`] renders a refusal
/// naming this code as the bare status it is.
pub const ERRORLESS_REFUSAL_CODE: &str = "";

/// The status both refusals are answered with.
pub const KEY_REFUSAL_STATUS: u16 = 400;

/// The code S3 answers a storage class the addressed bucket cannot host with, and the
/// sentence that code carries (same published list: "InvalidStorageClass — The storage class
/// you specified is not valid", HTTP 400).
pub const INVALID_STORAGE_CLASS_CODE: &str = "InvalidStorageClass";
/// See [`INVALID_STORAGE_CLASS_CODE`].
pub const INVALID_STORAGE_CLASS_MESSAGE: &str = "The storage class you specified is not valid";
/// See [`INVALID_STORAGE_CLASS_CODE`].
pub const INVALID_STORAGE_CLASS_STATUS: u16 = 400;

/// The `StorageClass` values a GENERAL PURPOSE bucket hosts.
///
/// The model's `StorageClass` enum publishes fifteen values
/// (`smithy_query(query="shape",shape="StorageClass")`), and `PutObjectRequest$StorageClass`
/// states which of them belong to another kind of bucket: "Directory buckets only support
/// `EXPRESS_ONEZONE` (the S3 Express One Zone storage class) in Availability Zones and
/// `ONEZONE_IA` (the S3 One Zone-Infrequent Access storage class) in Dedicated Local Zones"
/// and "Amazon S3 on Outposts only uses the OUTPOSTS Storage Class". So a general purpose
/// bucket hosts every published class but those two — `EXPRESS_ONEZONE`, which is a directory
/// bucket's, and `OUTPOSTS`, which is an S3 on Outposts bucket's.
///
/// HOSTING a class is not permission to WRITE one: a general purpose bucket holds objects in
/// every class here, including the ones only another AWS service places there
/// ([`SERVICE_PLACED_STORAGE_CLASSES`]), which a write may not name.
pub const GENERAL_PURPOSE_STORAGE_CLASSES: &[&str] = &[
    "STANDARD",
    "REDUCED_REDUNDANCY",
    "STANDARD_IA",
    "ONEZONE_IA",
    "INTELLIGENT_TIERING",
    "GLACIER",
    "DEEP_ARCHIVE",
    "GLACIER_IR",
    "SNOW",
    "FSX_OPENZFS",
    "FSX_ONTAP",
    "AWS_BACKUP_WARM",
    "AWS_BACKUP_LOW_COST_WARM",
];

/// The `StorageClass` values a DIRECTORY bucket hosts — the two that same member names for
/// one: `EXPRESS_ONEZONE` in an Availability Zone and `ONEZONE_IA` in a Dedicated Local Zone.
pub const DIRECTORY_BUCKET_STORAGE_CLASSES: &[&str] =
    &[state::EXPRESS_ONEZONE_STORAGE_CLASS, "ONEZONE_IA"];

/// The classes `bucket` can host, by the kind of bucket it is.
pub fn hosted_storage_classes(bucket: &Bucket) -> &'static [&'static str] {
    if bucket.is_directory() {
        DIRECTORY_BUCKET_STORAGE_CLASSES
    } else {
        GENERAL_PURPOSE_STORAGE_CLASSES
    }
}

/// The header a write names the class it asks for in, as `PutObjectRequest$StorageClass`,
/// `CopyObjectRequest$StorageClass` and `CreateMultipartUploadRequest$StorageClass` are each
/// bound (`smithy_query(query="operation",operation="PutObject")` reports
/// `httpHeader: "x-amz-storage-class"`) — the argument a refusal over its value names.
pub const STORAGE_CLASS_HEADER: &str = "x-amz-storage-class";

/// The `StorageClass` values a bucket HOLDS objects in but no client write may NAME: the
/// class is the one the AWS service that owns those objects stores them under, and nothing
/// else puts an object there.
///
/// Each is a published `StorageClass` value (`smithy_query(query="shape",shape="StorageClass")`)
/// whose own storage class description names the service that places it: `SNOW` is the class
/// of objects on a Snow Family device, `FSX_OPENZFS` and `FSX_ONTAP` of objects an Amazon FSx
/// file system holds, and the two `AWS_BACKUP_*` classes of objects AWS Backup wrote into a
/// vault. Membership in the enum is what the output of a read reports, not permission to ask
/// a write for it: S3 refuses a write naming one of these, whatever bucket it addresses, with
/// the argument refusal [`storage_class_refusal`] words.
pub const SERVICE_PLACED_STORAGE_CLASSES: &[&str] = &[
    "SNOW",
    "FSX_OPENZFS",
    "FSX_ONTAP",
    "AWS_BACKUP_WARM",
    "AWS_BACKUP_LOW_COST_WARM",
];

/// The fixed part of the sentence S3 answers a class a write may not name with: the class the
/// request named, then this text (recorded behaviour; the class is the request's own value).
pub const STORAGE_CLASS_NOT_ALLOWED: &str = " is not allowed.";

/// The refusal a write naming `storage_class` into `bucket` is answered with, or `None` for a
/// class that write may ask for (and for a write that names none, which is stored in the
/// bucket's own default).
///
/// Two separate rules, both judged before anything is written, so a refused write leaves
/// whatever the key held exactly as it was:
///
/// * a class no client write may name at all ([`SERVICE_PLACED_STORAGE_CLASSES`]) is a bad
///   ARGUMENT: HTTP 400 `InvalidArgument` whose message is that class followed by
///   [`STORAGE_CLASS_NOT_ALLOWED`], naming the header it arrived in and echoing the value, so
///   a caller reads which class it must stop asking for;
/// * a class that belongs to another KIND of bucket than the one addressed — `EXPRESS_ONEZONE`
///   and `OUTPOSTS` against a general purpose bucket, and everything but a directory bucket's
///   two against one of those — would store the object under a class the bucket cannot host,
///   which every later read, lifecycle transition and restore of it would then report. That
///   value is not valid for the request, which is what `InvalidStorageClass` reports
///   ([`invalid_storage_class`], the one envelope that class is answered with).
pub fn storage_class_refusal<E>(
    bucket: &Bucket,
    storage_class: Option<&str>,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    let named = storage_class?;
    if SERVICE_PLACED_STORAGE_CLASSES.contains(&named) {
        return Some(crate::scaffolded::Scaffolded::invalid_argument(
            STORAGE_CLASS_HEADER,
            Some(named),
            format!("{named}{STORAGE_CLASS_NOT_ALLOWED}"),
        ));
    }
    (!hosted_storage_classes(bucket).contains(&named)).then(|| invalid_storage_class(named))
}

/// The refusal S3 answers a write whose `x-amz-storage-class` names a class the request cannot
/// be served under with: [`INVALID_STORAGE_CLASS_CODE`] at [`INVALID_STORAGE_CLASS_STATUS`]
/// with that code's fixed sentence, plus `<StorageClassRequested>` echoing the class the
/// request asked for.
///
/// The member is the only place this class of refusal says WHICH class was rejected, since the
/// sentence names none (recorded response), so it travels with every such refusal: a value
/// outside the set the model publishes for the member ([`rejected_storage_class`] reads it
/// straight from the request's header, because such a value never reaches a deserialized
/// input) and a published value the addressed bucket cannot host are the same answer to the
/// caller — the class it named is not one this request can be served under.
///
/// The ONE place that envelope is built, so every write that names a class refuses it
/// identically.
pub fn invalid_storage_class<E>(requested: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::unmodeled(
        INVALID_STORAGE_CLASS_CODE,
        INVALID_STORAGE_CLASS_STATUS,
        INVALID_STORAGE_CLASS_MESSAGE,
    )
    .naming(crate::scaffolded::ErrorDetail::StorageClassRequested(
        requested.to_string(),
    ))
}

/// The refusal a write whose `x-amz-storage-class` is outside the set the model publishes for
/// the member it binds earns, or `None` for a request carrying a published value or no such
/// header at all.
///
/// The judgement the BOUNDARY would otherwise make, made where the refusal can carry S3's own
/// envelope: the boundary's refusal words the constraint the validating walk checked, naming
/// the Smithy member path and the model's value list, where S3 answers its own
/// `InvalidStorageClass` ([`invalid_storage_class`]) naming the class the request asked for —
/// so the value is withheld from the codec and read here from the request's own headers,
/// exactly as the two canned-ACL-shaped headers judged that way are
/// (`crate::service::wire::rejected_canned_acl`). The set comes from the OPERATION's own
/// member, so nothing outside it is ever stored.
pub fn rejected_storage_class<E>(
    parts: &::http::request::Parts,
    input_schema: &::aws_smithy_schema::Schema,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    crate::service::wire::rejected_storage_class(parts, input_schema).map(invalid_storage_class)
}

/// Why the object key `key` is refused, or `None` for a key S3 stores.
///
/// Two published rules about the NAME, both of them about the bytes it is made of:
///
/// * "The name for a key is a sequence of Unicode characters whose UTF-8 encoding is at most
///   1,024 bytes long" (Amazon S3 User Guide, *Creating object key names*) — a longer name is
///   `KeyTooLongError`, the code a client branching on the limit reads;
/// * a NUL (0x00) byte can be conveyed by no request URI, so a key carrying one is not a
///   request the API gets to refuse at all: it is answered with the bare 400 that carries no
///   error document ([`ERRORLESS_REFUSAL_CODE`]).
///
/// Judged before anything is stored, so a refused write leaves whatever the key held exactly
/// as it was.
pub fn key_refusal(key: &str) -> Option<KeyRefusal> {
    if key.contains('\0') {
        return Some(KeyRefusal::Unconveyable);
    }
    (key.len() > state::MAX_OBJECT_KEY_BYTES).then_some(KeyRefusal::TooLong { bytes: key.len() })
}

/// Why a key is refused — and therefore the whole envelope S3 answers it with, since what the
/// document carries differs by the class the refusal belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRefusal {
    /// A key no request URI can convey, which is refused before the API layer and so carries
    /// no error document at all ([`ERRORLESS_REFUSAL_CODE`]).
    Unconveyable,
    /// A key past the published ceiling: [`KEY_TOO_LONG_CODE`], whose document also states the
    /// two numbers its sentence does not — see [`KeyRefusal::detail`].
    TooLong {
        /// How many bytes the key the request named is made of.
        bytes: usize,
    },
}

impl KeyRefusal {
    /// The wire `<Code>` this refusal carries.
    pub fn code(self) -> &'static str {
        match self {
            Self::Unconveyable => ERRORLESS_REFUSAL_CODE,
            Self::TooLong { .. } => KEY_TOO_LONG_CODE,
        }
    }

    /// Its `<Message>`.
    pub fn message(self) -> &'static str {
        match self {
            Self::Unconveyable => "",
            Self::TooLong { .. } => KEY_TOO_LONG_MESSAGE,
        }
    }

    /// The members this class's error document carries BESIDE the sentence, or `None` for a
    /// class whose document carries none.
    ///
    /// A key past the ceiling is refused on its SIZE, and S3's document for that class carries
    /// the `<Size>` / `<MaxSizeAllowed>` pair — the key's own byte count and the ceiling it was
    /// judged against — because the sentence names neither, so the pair is the only place the
    /// refusal says by how much the caller has to shorten the name (recorded response; the same
    /// pair every size-refused value of this service carries,
    /// [`crate::scaffolded::ErrorDetail::Size`]).
    pub fn detail(self) -> Option<crate::scaffolded::ErrorDetail> {
        match self {
            Self::Unconveyable => None,
            Self::TooLong { bytes } => Some(crate::scaffolded::ErrorDetail::Size {
                size: i64::try_from(bytes).unwrap_or(i64::MAX),
                max_size_allowed: i64::try_from(state::MAX_OBJECT_KEY_BYTES).unwrap_or(i64::MAX),
            }),
        }
    }

    /// This refusal on the error channel every object write answers on, whole: the code, the
    /// status and the sentence, plus the members its document carries.
    ///
    /// The ONE place that envelope is built, so every write that names a key — the put, the
    /// copy, the upload initiation and completion, the rename and the browser form — refuses an
    /// unstorable name identically.
    pub fn scaffolded<E>(self) -> crate::scaffolded::Scaffolded<E> {
        let refusal = crate::scaffolded::Scaffolded::unmodeled(
            self.code(),
            KEY_REFUSAL_STATUS,
            self.message(),
        );
        match self.detail() {
            Some(detail) => refusal.naming(detail),
            None => refusal,
        }
    }
}

/// The sentence S3 answers a key past the published ceiling with: fixed text, naming neither
/// the key nor the ceiling (Amazon S3 "Error responses": "KeyTooLongError — Your key is too
/// long").
pub const KEY_TOO_LONG_MESSAGE: &str = "Your key is too long";

/// The refusal a write this emulator can no longer HOLD is answered with: the code S3
/// publishes for a request it is unable to handle ([`state::SERVICE_UNAVAILABLE_CODE`], HTTP
/// 503) carrying that code's own fixed sentence.
///
/// It is NOT a quota refusal, because S3 announces no quota over what is stored: a bucket
/// holds any number of objects, a key any number of versions, and `PutObject` and
/// `UploadPart` model no capacity error at all
/// (`smithy_query(query="operation",operation="PutObject,UploadPart")`). The condition is
/// this PROCESS having no more room for its object store
/// ([`state::MAX_STORED_VERSIONS_TOTAL`], [`state::MAX_RETAINED_BYTES_TOTAL`] — sized far
/// above the traffic the API admits, so no ordinary session meets them), which is a state of
/// the server rather than a fault in the request, and the code S3 publishes for that is the
/// one a client already retries.
///
/// The ONE place that envelope is built, so every write that can meet the guard — a put, a
/// copy, a part upload, a completion, a subresource write and the delete marker of a delete
/// — refuses identically, and nothing is stored when it does.
pub fn store_full<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::unmodeled(
        state::SERVICE_UNAVAILABLE_CODE,
        state::SERVICE_UNAVAILABLE_STATUS,
        state::SERVICE_UNAVAILABLE_MESSAGE,
    )
}

/// The query parameter that addresses the RENAME of an object: `?renameObject`, as the model
/// pins it on the operation's URI (`smithy_query(query="operation",operation="RenameObject")`
/// reports `http {method: "PUT", uri: "/{Bucket}/{Key+}?renameObject"}`).
pub const RENAME_OBJECT_PARAMETER: &str = "renameObject";

/// The query parameters of an object WRITE that S3 serves for DIRECTORY buckets alone.
///
/// `RenameObject` "renames an existing object in a directory bucket that uses the S3 Express
/// One Zone storage class" (its own documentation) and is addressed by the one parameter
/// above, so a general purpose bucket's endpoint implements no functionality for it: where the
/// parameter is all the request carries to ask for the rename, the PARAMETER, not the value in
/// it, is what cannot be served there — which is the 501 `NotImplemented` S3 answers a query
/// parameter it cannot honour with
/// ([`crate::scaffolded::Scaffolded::not_implemented_query_parameter`]), the query-parameter
/// sibling of the refusal a directory-bucket-only HEADER earns
/// ([`DIRECTORY_BUCKET_ONLY_HEADERS`], which is what a request carrying one is refused by
/// instead).
pub const DIRECTORY_BUCKET_ONLY_QUERY_PARAMETERS: [&str; 1] = [RENAME_OBJECT_PARAMETER];

/// The header a rename names its SOURCE in: `x-amz-rename-source`, as the model binds the one
/// member a request has to carry to ask for a rename at all
/// (`smithy_query(query="operation",operation="RenameObject")` reports
/// `RenameSource: {httpHeader: "x-amz-rename-source", required: true}`).
pub const RENAME_SOURCE_HEADER: &str = "x-amz-rename-source";

/// The headers of an object WRITE that S3 serves for DIRECTORY buckets alone.
///
/// A rename is asked for by `x-amz-rename-source`: the `@required` member above is the only
/// one a request cannot leave out and still name a rename, and S3 renames "an existing object
/// in a directory bucket that uses the S3 Express One Zone storage class" (`RenameObject`'s own
/// documentation) — so a general purpose bucket's endpoint implements nothing for that header,
/// whatever else the request carries.
pub const DIRECTORY_BUCKET_ONLY_HEADERS: [&str; 1] = [RENAME_SOURCE_HEADER];

/// The directory-bucket-only header `headers` carries, or `None` when it carries none.
pub fn directory_bucket_only_header(headers: &::http::HeaderMap) -> Option<&'static str> {
    DIRECTORY_BUCKET_ONLY_HEADERS
        .into_iter()
        .find(|header| headers.contains_key(*header))
}

/// The directory-bucket-only query parameter `query` carries, or `None` when it carries none
/// — the one rule every object write addressed to a general purpose bucket is judged by,
/// whichever operation the request's shape routes it to.
///
/// `query` is the raw query string of the request (`ctx.parts().uri.query()`), because the
/// parameter is a discriminator the matched operation's own members do not carry: a write
/// that names it reaches `PutObject` when it is shaped like one, and `RenameObject` when it
/// carries that operation's required members. Each name is looked for through the one reader
/// this crate asks a query string for a parameter with ([`crate::service::query_carries`]),
/// so a parameter arriving as a bare flag is found here exactly as it is everywhere else.
pub fn directory_bucket_only_query_parameter(query: &str) -> Option<&'static str> {
    DIRECTORY_BUCKET_ONLY_QUERY_PARAMETERS
        .into_iter()
        .find(|parameter| crate::service::query_carries(query, parameter))
}

/// The refusal an object write addressed to a GENERAL PURPOSE bucket earns for asking, through
/// a member S3 serves for directory buckets alone, for functionality that bucket's endpoint
/// does not implement — or `None` for a request that asks for none of it.
///
/// Either way the answer is the 501 `NotImplemented` S3 publishes for functionality it does not
/// implement for the resource addressed; what differs is the element the document names,
/// because the sentence names only the CLASS of request element and the element is the only
/// part of the refusal that says which one the caller must drop. So the refusal names what
/// ASKED:
///
/// * the HEADER, when the request carries one S3 serves for directory buckets alone
///   ([`DIRECTORY_BUCKET_ONLY_HEADERS`]) — a header is a member of the request, so it is what
///   the caller has to stop sending, and it is what S3's own answer to a rename addressed to a
///   general purpose bucket names even though the `?renameObject` parameter travels beside it;
/// * otherwise the QUERY PARAMETER ([`DIRECTORY_BUCKET_ONLY_QUERY_PARAMETERS`]), which is then
///   all that asks.
///
/// The ONE place that refusal is built for an object write, so a request carrying the same
/// member is refused identically whichever operation its shape routes it to, and before
/// anything is stored.
pub fn directory_bucket_only_refusal<E>(
    headers: &::http::HeaderMap,
    query: &str,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    if let Some(header) = directory_bucket_only_header(headers) {
        return Some(crate::scaffolded::Scaffolded::not_implemented_header(header));
    }
    directory_bucket_only_query_parameter(query)
        .map(crate::scaffolded::Scaffolded::not_implemented_query_parameter)
}

/// The server-side encryption settings a Zonal endpoint write into the directory bucket
/// `bucket` is governed by — the settings the write's own `x-amz-server-side-encryption` has
/// to match ([`session_encryption_refusal`]) and the ones it is STORED under when it names no
/// encryption member of its own — or `None` for a request that presents no live session of
/// that bucket, or a session that settled no encryption settings at all: neither commits
/// anything for a write to differ from or to be stored under.
///
/// "In the Zonal endpoint API calls … the encryption request headers must match the encryption
/// settings that are specified in the `CreateSession` request. … You don't need to explicitly
/// specify these encryption settings values in Zonal endpoint API calls, and Amazon S3 will
/// use the encryption settings values from the `CreateSession` request to protect new objects
/// in the directory bucket" (`PutObjectRequest$ServerSideEncryption`, of a directory bucket),
/// so it is the settings the SESSION the request presents was established with: a session that
/// named any were established under those, and one that named none under the bucket's own
/// default encryption configuration (`CreateSession` settles that, which is why this reads the
/// session rather than the bucket).
///
/// Read-only: the session is read, and nothing is written.
pub fn committed_encryption(
    store: &state::ComAmazonawsS3Amazons3State,
    bucket: &str,
    headers: &::http::HeaderMap,
    now: ::std::time::SystemTime,
) -> Option<state::ObjectEncryption> {
    let presented = state::presented_session(headers)?;
    let scope = store.presented_session_scope(&presented, now)?;
    if scope.bucket != bucket {
        return None;
    }
    (!scope.encryption.names_no_member()).then(|| scope.encryption.clone())
}

/// The sentence S3 refuses a write that asks the DIRECTORY bucket `bucket` to store an object
/// under a CUSTOMER-PROVIDED key with, or `None` for a write that names no such key or one into
/// a general purpose bucket, which serves them.
///
/// "This functionality is not supported for directory buckets"
/// (`PutObjectRequest$SSECustomerAlgorithm`), and "not supported when the destination bucket is
/// a directory bucket" (`CopyObjectRequest$SSECustomerAlgorithm`) — a directory bucket serves
/// SSE-S3 and SSE-KMS and no customer-provided key at all, so what refused the write is that
/// kind of bucket's own restriction, which is the sentence a directory bucket names a feature it
/// does not have in (`crate::service::directory_bucket::unsupported_message`) rather than a
/// member this emulator has not written.
///
/// Read off the three members that carry such a key, as every call of this service reads them
/// ([`state::requested_customer_key`] takes the same three): any of them names one, and it is
/// the ASK that is refused rather than whether the key is usable.
///
/// The ONE place that refusal is built, so a write carrying the same member is refused
/// identically whichever operation stores the object — and before anything is stored.
pub fn directory_bucket_customer_key_refusal(
    bucket: &Bucket,
    algorithm: Option<&str>,
    key: Option<&str>,
    key_md5: Option<&str>,
) -> Option<String> {
    let names_a_key = [algorithm, key, key_md5]
        .into_iter()
        .any(|member| member.is_some_and(|value| !value.is_empty()));
    (bucket.is_directory() && names_a_key).then(|| {
        crate::service::directory_bucket::unsupported_message(
            crate::service::directory_bucket::SSE_CUSTOMER_KEY,
        )
    })
}

/// The sentence S3 refuses a write that asks `bucket` to store the object `key` under the
/// CUSTOMER-PROVIDED key `presented` with when that bucket BLOCKS that encryption type, or
/// `None` for a write presenting no such key or into a bucket that serves it.
///
/// `BlockedEncryptionTypes` "prevent[s] the upload of new objects encrypted with the specified
/// server-side encryption type" (`ServerSideEncryptionRule$BlockedEncryptionTypes`), so what
/// refuses the write is the bucket's own setting rather than a missing grant: S3 answers the
/// authorization refusal (`AccessDenied`, HTTP 403) naming the principal, the write action, the
/// object it addressed and that setting as the reason
/// ([`state::SSE_C_BLOCKED_REASON`]). Whether the bucket blocks it is read off the rules in
/// force ([`Bucket::blocks_customer_key_encryption`]), the same ones `GetBucketEncryption`
/// reports.
///
/// Judged on the key the request's three SSE-C members really name, so a write whose members do
/// not name one usable key is refused for the member that does not hold
/// ([`state::requested_customer_key`]) and only a write that asks for this encryption type
/// reaches the setting at all.
///
/// The ONE place that refusal is built, so every write that reaches it is refused identically,
/// and before anything is stored: `PutObject` and `CreateMultipartUpload`, the two writes that
/// can ask for this encryption type today. `CopyObject` does not reach it — the DESTINATION's own
/// SSE-C members are still refused 501 for the member itself in that handler, so no bucket
/// setting can decide a copy that names one — and the calls of an upload already initiated
/// (`UploadPart`, `UploadPartCopy`, `CompleteMultipartUpload`) do not ask for the type at all:
/// they present the key the INITIATION recorded, and an initiation into a bucket that blocks it
/// is refused here, so no part of such an upload can exist.
pub fn blocked_customer_key_denial(
    bucket: &Bucket,
    principal: &str,
    action: &str,
    bucket_name: &str,
    key: &str,
    presented: Option<&state::CustomerKey>,
) -> Option<String> {
    (presented.is_some() && bucket.blocks_customer_key_encryption()).then(|| {
        state::authorization_denial(
            principal,
            action,
            &state::object_arn(bucket_name, key),
            state::SSE_C_BLOCKED_REASON,
        )
    })
}

/// The sentence S3 refuses a Zonal endpoint write naming `requested` under a session
/// established with `committed` with, or `None` when the two name one encryption.
///
/// "You can't override the values of the encryption settings … that are specified in the
/// `CreateSession` request. You don't need to explicitly specify these encryption settings
/// values in Zonal endpoint API calls, and Amazon S3 will use the encryption settings values
/// from the `CreateSession` request to protect new objects in the directory bucket"
/// (`PutObjectRequest$ServerSideEncryption`) — so a write that names NO algorithm is stored
/// under the session's own and matches it, and a write that names ANOTHER one names an
/// encryption the object cannot be stored under, which is a request S3 cannot serve
/// ([`state::INVALID_REQUEST_CODE`], HTTP 400).
///
/// The sentence is that class's fixed text with the two algorithms substituted, each named the
/// way S3 names an encryption type in it ([`encryption_type_name`]).
pub fn session_encryption_refusal(
    committed: Option<&str>,
    requested: Option<&str>,
) -> Option<String> {
    let requested = requested.filter(|algorithm| !algorithm.is_empty())?;
    let committed = committed.filter(|algorithm| !algorithm.is_empty())?;
    (committed != requested).then(|| {
        format!(
            "Session was created with ServerSideEncryption as {} but request header specifies \
             {}.",
            encryption_type_name(committed),
            encryption_type_name(requested)
        )
    })
}

/// The `ObjectCannedACL` values that are a BUCKET OWNER FULL CONTROL ACL: they give the
/// bucket owner `FULL_CONTROL` of the object and give nobody else anything.
///
/// The OBJECT rows of the published *Canned ACL* table (Amazon S3 User Guide, *Canned ACL*),
/// read for who each row grants to and with what: `private` gives the owner `FULL_CONTROL` and
/// "no one else has access rights", `bucket-owner-full-control` gives "both the object owner
/// and the bucket owner" `FULL_CONTROL`, and `bucket-owner-read` gives the bucket owner `READ`
/// beside the object owner's `FULL_CONTROL` — and under the Object Ownership setting that
/// disables ACLs the bucket owner IS the object owner, so each of the three leaves that one
/// grantee in full control and reaches nobody else. Every other value the member's enum
/// publishes (`smithy_query(query="shape",shape="ObjectCannedACL")`: `public-read`,
/// `public-read-write`, `authenticated-read`, `aws-exec-read`) grants to a predefined group or
/// to another AWS service's account, which is a third party.
///
/// Classified by the VALUE rather than by this emulator's expansion of it
/// ([`state::object_canned_acl_grants`]), because one published row's grantee — the Amazon EC2
/// account `aws-exec-read` grants `READ` to — has no canonical user id to expand to, so the
/// expansion of that row states fewer grantees than the row does.
pub const OWNER_FULL_CONTROL_OBJECT_CANNED_ACLS: [&str; 3] =
    ["private", "bucket-owner-read", "bucket-owner-full-control"];

/// Whether `grants` is a bucket owner full control ACL: it gives the bucket owner
/// `FULL_CONTROL` and gives nobody else anything.
///
/// This emulator serves one account ([`state::canonical_user_id`]), which owns every bucket
/// and — wherever the ownership setting that disables ACLs is in force — every object in one,
/// so a grant is the owner's own exactly when its grantee is that canonical user; a grant to a
/// predefined group, to an email address or to any other canonical user names somebody else.
/// `FULL_CONTROL` "allows grantee the READ, WRITE, READ_ACP, and WRITE_ACP permissions"
/// (Amazon S3 User Guide, *Access control list (ACL) overview*), so a further grant to that
/// same owner adds nothing to it.
pub fn grants_are_the_owners_full_control(grants: &[state::AclGrant]) -> bool {
    let owner = state::canonical_user_id();
    let the_owners = |grant: &state::AclGrant| grant.grantee.id.as_deref() == Some(owner.as_str());
    grants.iter().all(the_owners)
        && grants
            .iter()
            .any(|grant| the_owners(grant) && grant.permission == state::PERMISSION_FULL_CONTROL)
}

/// The grants the four explicit `x-amz-grant-*` headers of an object write describe, or `None`
/// when the write carries none of them.
///
/// Each header "specif[ies] access permissions explicitly" for one permission
/// (`PutObjectRequest$GrantRead` and its three siblings) and names its grantees as the
/// `type=value` pairs the one parse every grant header of this service is read by takes
/// ([`state::acl_grants_from_header`]). A header whose value names no grantee still NAMES an
/// ACL — the empty one, which leaves nobody in control of the object, so a bucket that admits
/// only a bucket owner full control ACL refuses it too.
pub fn grants_from_headers(
    full_control: Option<&str>,
    read: Option<&str>,
    read_acp: Option<&str>,
    write_acp: Option<&str>,
) -> Option<Vec<state::AclGrant>> {
    let mut grants: Vec<state::AclGrant> = Vec::new();
    let mut named = false;
    for (value, permission) in [
        (full_control, state::PERMISSION_FULL_CONTROL),
        (read, state::PERMISSION_READ),
        (read_acp, state::PERMISSION_READ_ACP),
        (write_acp, state::PERMISSION_WRITE_ACP),
    ] {
        let Some(value) = value else { continue };
        named = true;
        grants.extend(state::acl_grants_from_header(value, permission).unwrap_or_default());
    }
    named.then_some(grants)
}

/// The refusal an object write that NAMES an access control list earns on a bucket whose
/// Object Ownership setting disables ACLs, or `None` when that bucket still accepts the ACL —
/// and `None` for every bucket whose ACLs are enabled, which accepts any of them.
///
/// "Buckets that use this setting only accept PUT requests that don't specify an ACL or PUT
/// requests that specify bucket owner full control ACLs … PUT requests that contain other ACLs
/// … fail and return a 400 error with the error code `AccessControlListNotSupported`"
/// (`PutObjectRequest$ACL`, and `CopyObjectRequest$ACL` says the same of a copy): what such a
/// bucket admits is an ACL leaving the bucket owner in full control of the object and granting
/// nobody else anything, and every other ACL is refused. The ACL the request DESCRIBES decides
/// it, whichever of the member's two forms it was spelled in — a canned value
/// ([`OWNER_FULL_CONTROL_OBJECT_CANNED_ACLS`]) or the explicit grant headers
/// ([`grants_are_the_owners_full_control`]) — and a request naming both forms is refused for
/// that before it reaches here.
///
/// The ONE place that gate lives for every write that can carry the member, so `PutObject`,
/// `CopyObject` and `CreateMultipartUpload` admit and refuse exactly the same ACLs, and each
/// asks it before anything is stored, so a write S3 would not serve leaves behind neither an
/// object nor an upload.
pub fn disabled_acl_refusal<E>(
    bucket: &Bucket,
    canned: Option<&str>,
    grants: Option<&[state::AclGrant]>,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    if !bucket.acls_disabled() {
        return None;
    }
    let admitted = match canned {
        Some(canned) => OWNER_FULL_CONTROL_OBJECT_CANNED_ACLS.contains(&canned),
        None => grants.is_none_or(grants_are_the_owners_full_control),
    };
    (!admitted).then(|| {
        crate::scaffolded::Scaffolded::unmodeled(
            state::ACL_NOT_SUPPORTED_CODE,
            state::ACL_REFUSAL_STATUS,
            state::ACL_NOT_SUPPORTED_MESSAGE,
        )
    })
}

/// How S3 names a server-side encryption type in that sentence: the published SHORT NAME of
/// each KMS form — "server-side encryption with KMS keys (SSE-KMS) (`aws:kms`)" and its
/// dual-layer sibling (`PutObjectRequest$ServerSideEncryption`) — and the model's own
/// `ServerSideEncryption` value for every other, which for SSE-S3 is the algorithm name
/// `AES256` itself.
fn encryption_type_name(algorithm: &str) -> &str {
    match algorithm {
        kms if kms == state::KMS_SSE_ALGORITHMS[0] => "SSE-KMS",
        dsse if dsse == state::KMS_SSE_ALGORITHMS[1] => "DSSE-KMS",
        named => named,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogs this module divides the model's enum into, so neither can drift from it:
    /// every class a general purpose bucket hosts and every class no write may name is a
    /// published `StorageClass` value, the service-placed ones are a strict subset of the
    /// hosted ones (a bucket HOLDS them, a write just cannot ask for them), and the two
    /// bucket-kind catalogs leave no published value unclassified.
    #[test]
    fn every_classified_storage_class_is_one_the_model_publishes() {
        let published = crate::ops::types::STORAGE_CLASS_VALUES;
        for class in GENERAL_PURPOSE_STORAGE_CLASSES
            .iter()
            .chain(DIRECTORY_BUCKET_STORAGE_CLASSES)
            .chain(SERVICE_PLACED_STORAGE_CLASSES)
        {
            assert!(
                published.contains(class),
                "case {class:?}: every classified class must be a published StorageClass \
                 value, which {published:?} does not hold"
            );
        }
        for class in SERVICE_PLACED_STORAGE_CLASSES {
            assert!(
                GENERAL_PURPOSE_STORAGE_CLASSES.contains(class),
                "case {class:?}: a service-placed class is still one a general purpose bucket \
                 HOLDS objects in, got {GENERAL_PURPOSE_STORAGE_CLASSES:?}"
            );
        }
        assert!(
            GENERAL_PURPOSE_STORAGE_CLASSES.len() > SERVICE_PLACED_STORAGE_CLASSES.len(),
            "a write must be left some class it may name, got {SERVICE_PLACED_STORAGE_CLASSES:?} \
             of {GENERAL_PURPOSE_STORAGE_CLASSES:?}"
        );
        for class in published {
            assert!(
                GENERAL_PURPOSE_STORAGE_CLASSES.contains(class)
                    || DIRECTORY_BUCKET_STORAGE_CLASSES.contains(class)
                    || *class == "OUTPOSTS",
                "case {class:?}: every published class belongs to a kind of bucket — the \
                 Outposts one is served by no bucket this emulator holds"
            );
        }
    }

    /// The canned-ACL catalog this module classifies by, so it cannot drift from the model's
    /// own enum: every value admitted on a bucket that allows no ACLs is a published
    /// `ObjectCannedACL`, and the published values it leaves out are the ones granting to
    /// somebody other than the bucket owner, each of which such a bucket refuses.
    #[test]
    fn every_owner_full_control_canned_acl_is_one_the_model_publishes() {
        let published = crate::ops::types::OBJECT_CANNED_A_C_L_VALUES;
        for canned in OWNER_FULL_CONTROL_OBJECT_CANNED_ACLS {
            assert!(
                published.contains(&canned),
                "case {canned:?}: every admitted canned ACL must be a published \
                 ObjectCannedACL value, which {published:?} does not hold"
            );
        }
        let refused: Vec<&&str> = published
            .iter()
            .filter(|canned| !OWNER_FULL_CONTROL_OBJECT_CANNED_ACLS.contains(canned))
            .collect();
        assert_eq!(
            refused,
            vec![
                &"authenticated-read",
                &"aws-exec-read",
                &"public-read",
                &"public-read-write"
            ],
            "the values left over must be exactly the published ones granting beyond the \
             bucket owner, got {refused:?} of {published:?}"
        );
    }

    /// The key rules, at the ceiling and one byte past it, and over a key carrying the one
    /// byte no URI conveys — with the members each class's error document carries.
    #[test]
    fn a_key_is_refused_for_its_bytes_alone() {
        let rows: [(&str, String, Option<&'static str>); 6] = [
            ("a short key", "reports/held.csv".to_string(), None),
            ("a one-byte key", "k".to_string(), None),
            (
                "a key of exactly the published maximum",
                "k".repeat(state::MAX_OBJECT_KEY_BYTES),
                None,
            ),
            (
                "a key one byte past the maximum",
                "k".repeat(state::MAX_OBJECT_KEY_BYTES + 1),
                Some(KEY_TOO_LONG_CODE),
            ),
            (
                "a multi-byte key of exactly the published maximum in BYTES",
                "é".repeat(state::MAX_OBJECT_KEY_BYTES / 2),
                None,
            ),
            (
                "a key carrying a NUL byte",
                "nul/a\0b.txt".to_string(),
                Some(ERRORLESS_REFUSAL_CODE),
            ),
        ];
        for (case, key, expected) in rows {
            let refusal = key_refusal(&key);
            assert_eq!(
                refusal.map(KeyRefusal::code),
                expected,
                "case {case:?}: expected {expected:?}, got {refusal:?}"
            );
            // A key refused for its LENGTH states the two numbers its sentence does not: its
            // own byte count and the published ceiling. Nothing else carries them.
            let stated = refusal.and_then(KeyRefusal::detail).map(|detail| match detail {
                crate::scaffolded::ErrorDetail::Size {
                    size,
                    max_size_allowed,
                } => (size, max_size_allowed),
                other => panic!("case {case:?}: a key refusal states a size, got {other:?}"),
            });
            let owed = (expected == Some(KEY_TOO_LONG_CODE)).then(|| {
                (
                    i64::try_from(key.len()).expect("a test key fits an i64"),
                    i64::try_from(state::MAX_OBJECT_KEY_BYTES).expect("the ceiling fits an i64"),
                )
            });
            assert_eq!(
                stated, owed,
                "case {case:?}: expected the <Size>/<MaxSizeAllowed> pair {owed:?}, got \
                 {stated:?}"
            );
        }
    }
}
