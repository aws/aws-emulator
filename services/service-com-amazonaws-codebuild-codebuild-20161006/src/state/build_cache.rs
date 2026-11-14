// SPDX-License-Identifier: CC0-1.0

//! The CACHE one project carries from one of its builds into the next.
//!
//! # The rule this module implements
//!
//! A project whose cache is held in the account's object store carries the paths its
//! buildspec declares across builds: `ProjectCache$type` `S3` means "the build project
//! reads and writes from and to S3", `location` is "the Amazon S3 bucket name and prefix",
//! and the buildspec's `cache/paths` are "the locations … that CodeBuild uses to upload and
//! download the cache". So a build RESTORES those paths into its working tree before its
//! first command runs, and SAVES them again when it has run, which is what makes a file one
//! build cached readable by the next build of the same project.
//!
//! The cache is one object per cached scope, because the scope is what AWS says it is:
//! "cacheNamespace … defines the scope of the cache. You can use this namespace to share a
//! cache across multiple projects", so a project with no namespace caches under its own name
//! and projects that share a namespace share the object. `InvalidateProjectCache`
//! "[r]esets the cache for a project", so a build started after that call restores nothing
//! from a cache saved before it, however long the object stays in the bucket.
//!
//! A cache the store does not serve is simply no cache: the build runs with what it has
//! rather than failing — nothing a cache does is part of what a build is asked to produce —
//! and the paths it then saves become the cache the next build restores.

use ::smithy_server_runtime::EmbeddedEmulator;

use super::source_archive;
use super::{ArtifactDeclaration, CACHE_TYPE_NO_CACHE};
use crate::ops::types::ProjectCache;

/// The `CacheType` whose cache is held in the account's object store: "S3: The build
/// project reads and writes from and to S3" (`ProjectCache$type`).
pub const CACHE_TYPE_S3: &str = "S3";

/// How many bytes of a cache object a build reads: the cache is written by this service,
/// and the ceiling on what is buffered to read it is still the emulator's, exactly as the
/// one on a build's source archive is.
const MAX_CACHE_BYTES: usize = 4 * 1024 * 1024;

/// The object one project's cache lives at: the bucket the cache's `location` names and
/// the key under it — `None` for a project whose cache is not held in the object store, or
/// whose cache names no bucket.
///
/// The key is the prefix the location carries, then the scope the cache is shared at: the
/// `cacheNamespace` when the project declares one, and the project's own name otherwise.
pub fn cache_object(cache: &ProjectCache, project_name: &str) -> Option<(String, String)> {
    if cache.r#type != CACHE_TYPE_S3 {
        return None;
    }
    let location = cache.location.as_deref()?.trim();
    let (bucket, prefix) = match location.split_once('/') {
        Some((bucket, prefix)) => (bucket.trim(), prefix.trim()),
        None => (location, ""),
    };
    if bucket.is_empty() {
        return None;
    }
    let scope = cache
        .cache_namespace
        .as_deref()
        .map(str::trim)
        .filter(|namespace| !namespace.is_empty())
        .unwrap_or(project_name);
    let key = [prefix, scope]
        .iter()
        .map(|part| part.trim_matches('/'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<&str>>()
        .join("/");
    (!key.is_empty()).then(|| (bucket.to_string(), key))
}

/// Whether `cache` is a cache at all — a project that configures none reports the type
/// that means no cache is used ([`CACHE_TYPE_NO_CACHE`]).
pub fn is_configured(cache: &ProjectCache) -> bool {
    cache.r#type != CACHE_TYPE_NO_CACHE
}

/// Restore one project's cached paths out of the object store, as the files to seed the
/// build's working tree with.
///
/// Empty for every answer that is not the store serving a cache object it wrote: there was
/// no cache to restore, so the build runs with what it has.
pub async fn restore(
    callback: &EmbeddedEmulator,
    region: &str,
    cache: &ProjectCache,
    project_name: &str,
) -> Vec<(String, String)> {
    let Some((bucket, key)) = cache_object(cache, project_name) else {
        return Vec::new();
    };
    let read = ::service_common::s3_object_read::read_object(
        callback,
        region,
        &bucket,
        &key,
        None,
        MAX_CACHE_BYTES,
    )
    .await;
    match read {
        ::service_common::s3_object_read::ObjectRead::Served(bytes) => {
            source_archive::unpacked(&bytes).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

/// Save the paths `declared` names out of `produced` as this project's cache, answering
/// whether the store took it.
///
/// A project whose buildspec declares no cached path caches nothing: there is no object to
/// write, which is answered as nothing saved rather than as a failure.
pub async fn save(
    callback: &EmbeddedEmulator,
    region: &str,
    cache: &ProjectCache,
    project_name: &str,
    declared: &[String],
    produced: &[(String, String)],
) -> bool {
    let Some((bucket, key)) = cache_object(cache, project_name) else {
        return false;
    };
    // The cached paths are selected the way a declared artifact's files are: the same
    // patterns, matched against the same working tree.
    let cached = super::artifact_upload::declared_files(
        &ArtifactDeclaration {
            files: declared.to_vec(),
            ..ArtifactDeclaration::default()
        },
        produced,
    );
    if cached.is_empty() {
        return false;
    }
    let Some(archive) = source_archive::packed(&cached) else {
        return false;
    };
    let put = ::service_common::s3_client::s3_client(callback, region)
        .put_object()
        .bucket(&bucket)
        .key(&key)
        .body(::aws_sdk_s3::primitives::ByteStream::from(archive))
        .send()
        .await;
    match put {
        Ok(_) => true,
        Err(error) => {
            ::tracing::warn!(
                %bucket,
                %key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a build's cache object was not taken by the emulator's Amazon S3"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cache as a row describes it.
    fn cache(kind: &str, location: Option<&str>, namespace: Option<&str>) -> ProjectCache {
        ProjectCache {
            r#type: kind.to_string(),
            location: location.map(str::to_string),
            modes: None,
            cache_namespace: namespace.map(str::to_string),
        }
    }

    /// RULE: a project's cache object is the prefix its location names followed by the
    /// scope the cache is shared at — the declared namespace, or the project's own name.
    ///
    /// The rows differ along what the rule generalises over: whether the location carries a
    /// prefix, whether a namespace is declared, and the cache types that name no object.
    #[test]
    fn a_cache_object_is_the_location_and_the_scope() {
        /// One row: the case, the cache, and the bucket and key it resolves to.
        type Row = (&'static str, ProjectCache, Option<(&'static str, &'static str)>);
        let rows: Vec<Row> = vec![
            (
                "a bucket and a prefix",
                cache(CACHE_TYPE_S3, Some("cache-bucket/caches"), None),
                Some(("cache-bucket", "caches/a-project")),
            ),
            (
                "the bucket alone",
                cache(CACHE_TYPE_S3, Some("cache-bucket"), None),
                Some(("cache-bucket", "a-project")),
            ),
            (
                "a namespace shared across projects",
                cache(CACHE_TYPE_S3, Some("cache-bucket/caches"), Some("shared")),
                Some(("cache-bucket", "caches/shared")),
            ),
            (
                "a cache held by the build host instead",
                cache("LOCAL", Some("cache-bucket"), None),
                None,
            ),
            (
                "no cache at all",
                cache(CACHE_TYPE_NO_CACHE, None, None),
                None,
            ),
            (
                "a cache in the store that names no bucket",
                cache(CACHE_TYPE_S3, None, None),
                None,
            ),
        ];
        for (case, cache, expected) in rows {
            let object = cache_object(&cache, "a-project");
            let read = object
                .as_ref()
                .map(|(bucket, key)| (bucket.as_str(), key.as_str()));
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}
