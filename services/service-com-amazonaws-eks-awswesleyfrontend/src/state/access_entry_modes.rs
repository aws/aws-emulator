// SPDX-License-Identifier: CC0-1.0

//! The authentication mode a cluster's access-entry API is served under.
//!
//! A cluster authenticates Kubernetes callers either through the `aws-auth`
//! ConfigMap inside the cluster or through the access entries Amazon EKS keeps
//! for it, and `accessConfig.authenticationMode` is which
//! (`AuthenticationMode`: `CONFIG_MAP`, `API_AND_CONFIG_MAP`, `API`). The
//! access-entry API acts on the second of those, so a cluster still on
//! `CONFIG_MAP` alone has no access-entry plane for it to act on: every
//! operation of that API is refused rather than answered, which is what tells a
//! caller the cluster is still `aws-auth`-based instead of letting it read an
//! empty list and conclude the API mode is active.

use super::*;

/// The `authenticationMode` members under which Amazon EKS serves a cluster's
/// access-entry API: the two that include the API
/// ([`AUTHENTICATION_MODE_LADDER`] is the whole set, `CONFIG_MAP` the one left
/// out).
pub const ACCESS_ENTRY_AUTHENTICATION_MODES: &[&str] = &["API", "API_AND_CONFIG_MAP"];

/// The sentence an access-entry call on a cluster that serves no access-entry
/// API is refused with.
///
/// It states the RULE — the modes that do serve it — and carries nothing from
/// the request: what the caller has to change is the cluster's mode, and the
/// modes are the service's own ([`ACCESS_ENTRY_AUTHENTICATION_MODES`]), so the
/// list is rendered from them rather than written out a second time.
pub fn access_entry_authentication_mode_refusal() -> String {
    format!(
        "The cluster's authentication mode must be set to one of [{}] to perform this \
         operation.",
        ACCESS_ENTRY_AUTHENTICATION_MODES.join(", ")
    )
}

impl ClusterRecord {
    /// The `authenticationMode` this cluster authenticates its Kubernetes callers
    /// under.
    ///
    /// The stored value, which `CreateCluster` writes for every cluster — the one
    /// the request asked for, or [`DEFAULT_AUTHENTICATION_MODE`] when it asked for
    /// none — so a record whose `accessConfig` carries none is read as that
    /// default rather than as having no mode.
    pub fn authentication_mode(&self) -> &str {
        self.access_config
            .as_ref()
            .and_then(|config| config.authentication_mode.as_deref())
            .unwrap_or(DEFAULT_AUTHENTICATION_MODE)
    }

    /// Whether this cluster serves the access-entry API at all.
    ///
    /// The one reading of the gate, so every operation of that API applies it
    /// identically: the access entries Amazon EKS keeps are what the API acts on,
    /// and a cluster authenticating only through the `aws-auth` ConfigMap keeps
    /// none for it to act on.
    pub fn serves_access_entries(&self) -> bool {
        ACCESS_ENTRY_AUTHENTICATION_MODES.contains(&self.authentication_mode())
    }
}
