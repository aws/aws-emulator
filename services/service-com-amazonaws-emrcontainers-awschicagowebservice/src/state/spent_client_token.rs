// SPDX-License-Identifier: CC0-1.0

//! The ONE decision a `clientToken` already spent in its scope is judged by, for every
//! operation of this service whose input declares an `@idempotencyToken` `clientToken`.
//!
//! `@idempotencyToken` means "the same token names the same request": a call carrying a
//! token already spent in its scope is a REPLAY — answered with the resource the first
//! call minted, so a retry never doubles the resource — only when it really is the same
//! request ([`super::request_fingerprint`] is how the two are told apart); a call carrying
//! a DIFFERENT request under that token is not a replay, because answering it with the
//! earlier resource would silently drop the parameters this call asked for, so it is
//! refused. A token no call has spent in this scope is a fresh request.
//!
//! The index lookup ([`super::ComAmazonawsEmrcontainersAwschicagowebserviceState::replayed_idempotency`])
//! and the digest already lived in one place; this is the DECIDING half, so the three
//! outcomes a spent token can have are written once for the whole crate rather than once
//! per handler. A handler keeps what is its own — which store the id is resolved in, its
//! modelled error and its wording, and the output shape it answers with — and takes the
//! decision from here, so no handler can lose the request comparison on its own and start
//! answering a different request with an earlier resource.

use super::{
    ComAmazonawsEmrcontainersAwschicagowebserviceState, EndpointSessionRecord, JobRunRecord,
    JobTemplateRecord, ManagedEndpointRecord, SecurityConfigurationRecord, VirtualClusterRecord,
};

/// A stored record a spent `clientToken` can name: it carries the digest of the request
/// that minted it, which is what decides whether a later call under the same token is that
/// same request.
///
/// Implemented by each record an `@idempotencyToken` operation of this service persists, so
/// [`ComAmazonawsEmrcontainersAwschicagowebserviceState::spent_client_token`] — not one copy
/// per handler — is the only place the comparison is written.
pub trait IdempotentRecord {
    /// [`super::request_fingerprint`] of the request that minted this record.
    fn request_fingerprint(&self) -> &str;
}

impl IdempotentRecord for VirtualClusterRecord {
    fn request_fingerprint(&self) -> &str {
        &self.request_fingerprint
    }
}

impl IdempotentRecord for JobRunRecord {
    fn request_fingerprint(&self) -> &str {
        &self.request_fingerprint
    }
}

impl IdempotentRecord for JobTemplateRecord {
    fn request_fingerprint(&self) -> &str {
        &self.request_fingerprint
    }
}

impl IdempotentRecord for SecurityConfigurationRecord {
    fn request_fingerprint(&self) -> &str {
        &self.request_fingerprint
    }
}

impl IdempotentRecord for ManagedEndpointRecord {
    fn request_fingerprint(&self) -> &str {
        &self.request_fingerprint
    }
}

impl IdempotentRecord for EndpointSessionRecord {
    fn request_fingerprint(&self) -> &str {
        &self.request_fingerprint
    }
}

/// What the `clientToken` a request carries means for the call now being served.
#[derive(Debug)]
pub enum SpentClientToken<'a, R> {
    /// The token names no record this scope resolves, so this call is a first call: the
    /// handler goes on to mint the resource.
    Fresh,
    /// The token was spent by the SAME request, which minted this record: the handler
    /// answers with it and mints nothing.
    Replay(&'a R),
    /// The token was spent by a DIFFERENT request: the handler refuses this call with its
    /// own modelled `ValidationException`, carrying
    /// [`super::IDEMPOTENCY_CONFLICT_MESSAGE`].
    Conflict,
}

impl ComAmazonawsEmrcontainersAwschicagowebserviceState {
    /// The one `@idempotencyToken` rule of this crate, decided for one call.
    ///
    /// `scope` is the scope the token is remembered under (the operation, and the parent
    /// resource where the token identifies one request to it), `fingerprint` the digest of
    /// the request being served ([`super::request_fingerprint`]), and `resolve` the
    /// caller's own region-scoped lookup of the record an id names — the only part of the
    /// rule that differs between operations, since each stores its resource in its own map.
    ///
    /// A token whose id resolves to NO record is answered [`SpentClientToken::Fresh`]: the
    /// resource it named is gone (deleted, or reclaimed with its virtual cluster), so there
    /// is nothing to replay and the token is spendable again.
    pub fn spent_client_token<'state, R: IdempotentRecord>(
        &'state self,
        region: &str,
        scope: &str,
        client_token: &str,
        fingerprint: &str,
        resolve: impl FnOnce(&'state Self, &str) -> Option<&'state R>,
    ) -> SpentClientToken<'state, R> {
        let Some(minted) = self.replayed_idempotency(region, scope, client_token) else {
            return SpentClientToken::Fresh;
        };
        let Some(existing) = resolve(self, minted) else {
            return SpentClientToken::Fresh;
        };
        if existing.request_fingerprint() != fingerprint {
            return SpentClientToken::Conflict;
        }
        SpentClientToken::Replay(existing)
    }
}
