// SPDX-License-Identifier: CC0-1.0

//! The CODE an agent runtime's `codeConfiguration` artifact names, read out of the
//! object storage that holds it.
//!
//! A runtime whose artifact is a `codeConfiguration` RUNS the code at the
//! object-storage location that configuration names: `code` is "the source code
//! location", and `prefix` is "the prefix for objects in the Amazon S3 bucket"
//! (`smithy_query shape="CodeConfiguration,S3Location"`). Whether an object is there
//! at all is resolved when the runtime version is WRITTEN ([`super::code_object_is_absent`]);
//! what is read HERE is the artifact's own bytes, which is what the data plane hands to
//! the engine that runs the deployed code.
//!
//! The read lives in this crate because the agent runtime is this service's resource: the
//! location is held on the runtime's version record, so the plane that invokes a runtime asks
//! the component that OWNS it for the artifact's bytes rather than reading that member for
//! itself. The read itself is the one this tree makes of any object
//! ([`::service_common::s3_object_read`]) over the in-process hop into this emulator's own
//! Amazon S3 ([`::service_common::s3_client`]), so what the store's answer MEANT is read the
//! same way here as everywhere else.
//!
//! Two reads, both of them reads: nothing in this service's store or in object storage is
//! changed by them.
//!
//! * the key the location names, which is the artifact when the prefix addresses ONE
//!   stored object (a single module, or an archive holding the agent's files);
//! * otherwise the keys UNDER that prefix, because the member is modelled as a prefix: an
//!   object stored under it is part of the artifact too, so each is read and keyed by its
//!   path under the prefix.
//!
//! What one read may hold is bounded here, in host code: object storage holds whatever a
//! caller put there, so [`MAX_CODE_OBJECTS`] objects and [`MAX_CODE_BYTES`] bytes are read
//! at most and the rest is left unread.
//!
//! Re-exported from [`crate::state`], so a handler names `crate::state::…` for these
//! exactly as it does for the runtime they belong to.

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;
use ::service_common::s3_object_read::{read_object, ObjectRead};

use crate::ops::types::S3Location;

/// How many objects of one artifact are read.
///
/// An EMULATOR bound, not an AWS one: a prefix may hold any number of keys, and the
/// artifact is read into this process's memory, so the count one invocation can make this
/// host hold is stated here. Far above the handful of files an agent is deployed as.
pub const MAX_CODE_OBJECTS: usize = 256;

/// How many bytes of one artifact are read, across every object of it.
///
/// An EMULATOR bound for the reason [`MAX_CODE_OBJECTS`] is one, on the size side: it is
/// what keeps the memory one invocation makes this process hold proportional to a
/// deployment package rather than to what a caller chose to store under the prefix.
pub const MAX_CODE_BYTES: usize = 16 * 1024 * 1024;

/// The character an object key's path segments are separated by, which is what makes a
/// prefix name a set of keys rather than one key.
const KEY_SEPARATOR: &str = "/";

/// One object of an artifact.
#[derive(Debug, Clone)]
pub struct CodeObject {
    /// The object's path under the artifact's prefix — its own key for a prefix that
    /// addresses one stored object — which is the path the deployed code's entry point is
    /// named by.
    pub path: String,
    /// The bytes object storage holds for it.
    pub bytes: Vec<u8>,
}

/// Why the code an artifact names could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeUnavailable {
    /// Object storage holds nothing at the location the artifact names, so there is no
    /// deployed code to run.
    NotHeld(String),
    /// Object storage could not be read at all — an emulator assembled without it, a
    /// transport that answered nothing — so what is at the location is unknown.
    Unreadable(String),
}

/// The objects object storage holds for the code `location` names, keyed by their path
/// under its prefix.
///
/// `Ok` carries at least one object: a location that resolves to no object at all is
/// [`CodeUnavailable::NotHeld`], because there is nothing deployed there to run.
pub async fn artifact_code(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &S3Location,
) -> Result<Vec<CodeObject>, CodeUnavailable> {
    // The ONE read of an object this tree makes ([`::service_common::s3_object_read`]), over
    // the in-process hop into this emulator's own Amazon S3, bounded by what one artifact may
    // hold: so what the store's answer MEANT is read the same way here as everywhere else.
    match read_object(
        ctx.callback(),
        region,
        &location.bucket,
        &location.prefix,
        location.version_id.as_deref(),
        MAX_CODE_BYTES,
    )
    .await
    {
        ObjectRead::Served(bytes) => {
            return Ok(vec![CodeObject {
                path: file_name_of(&location.prefix),
                bytes,
            }])
        }
        // A location that names a VERSION names ONE stored object rather than a set, so that
        // read was the whole answer; one that names none leaves the prefix to resolve.
        ObjectRead::Absent if location.version_id.is_none() => {}
        ObjectRead::Absent => return Err(CodeUnavailable::NotHeld(code_absent(location))),
        ObjectRead::Denied => {
            return Err(CodeUnavailable::NotHeld(format!(
                "object storage does not permit this account to read the code object at s3://{}/{}",
                location.bucket, location.prefix
            )))
        }
        ObjectRead::Unreadable | ObjectRead::Unanswered => {
            return Err(CodeUnavailable::Unreadable(format!(
                "the agent runtime's code object at s3://{}/{} could not be read",
                location.bucket, location.prefix
            )))
        }
    }
    // What is left is the prefix standing for a SET of keys. It is listed at its own path
    // separator — the prefix plus `/` unless it already ends in one — so a key that merely
    // shares the value as a STRING (`<prefix>.bak`) is not taken for the artifact, while
    // every key under the prefix is.
    let under_prefix = if location.prefix.ends_with(KEY_SEPARATOR) {
        location.prefix.clone()
    } else {
        format!("{}{KEY_SEPARATOR}", location.prefix)
    };
    let listed = ::service_common::s3_client::s3_client(ctx.callback(), region)
        .list_objects_v2()
        .bucket(&location.bucket)
        .prefix(&under_prefix)
        .max_keys(i32::try_from(MAX_CODE_OBJECTS).unwrap_or(i32::MAX))
        .send()
        .await
        .map_err(|failure| {
            CodeUnavailable::Unreadable(format!(
                "the keys under the agent runtime's code prefix could not be listed: {}",
                failure.code().unwrap_or("no error code")
            ))
        })?;
    let mut held: Vec<CodeObject> = Vec::new();
    let mut taken = 0usize;
    for object in listed.contents() {
        let Some(key) = object.key() else { continue };
        if held.len() >= MAX_CODE_OBJECTS || taken >= MAX_CODE_BYTES {
            break;
        }
        let Some(path) = key.strip_prefix(under_prefix.as_str()) else {
            continue;
        };
        if path.is_empty() || path.ends_with(KEY_SEPARATOR) {
            // A key that is the prefix itself, or one standing for a directory, carries no
            // file of the artifact.
            continue;
        }
        // Each object of the artifact is read under what is LEFT of the artifact's own bound,
        // so a prefix holding many objects is read up to that bound and no further.
        let room = MAX_CODE_BYTES.saturating_sub(taken);
        let ObjectRead::Served(bytes) =
            read_object(ctx.callback(), region, &location.bucket, key, None, room).await
        else {
            continue;
        };
        taken = taken.saturating_add(bytes.len());
        held.push(CodeObject {
            path: path.to_string(),
            bytes,
        });
    }
    if held.is_empty() {
        return Err(CodeUnavailable::NotHeld(code_absent(location)));
    }
    Ok(held)
}

/// The file name a key ends in, which is the path the deployed code's entry point names it
/// by; a key that is all separators is taken whole.
fn file_name_of(key: &str) -> String {
    let named = key.rsplit(KEY_SEPARATOR).next().unwrap_or(key);
    if named.is_empty() {
        key.to_string()
    } else {
        named.to_string()
    }
}

/// How a location that resolves to no object at all is reported: the bucket and the prefix
/// the artifact names, which is what a client has to fix.
fn code_absent(location: &S3Location) -> String {
    format!(
        "object storage holds no code object at s3://{}/{}",
        location.bucket, location.prefix
    )
}
