// SPDX-License-Identifier: CC0-1.0

//! WHO JUDGES the `@connections` endpoint a request was addressed through: whether the
//! invoke path in front of the modelled binding names a stage an api of this emulator has
//! DEPLOYED.
//!
//! The three `@connections` operations are served on the OWNING api's own invoke endpoint —
//! `https://{api-id}.execute-api.{region}.amazonaws.com/{stage}/@connections/{id}`, "the
//! stage name is typically appended to this URI to form a complete path to a deployed API
//! stage" (`Api$ApiEndpoint`) — so the stage is part of every such request, while the apis
//! and the stages themselves are API Gateway V2's records. The management API cannot read
//! that store: API Gateway V2 already depends on the management API for the connection
//! registry, so the dependency cannot also run the other way.
//!
//! So the READING lives here, as a function the owner of apis and stages INSTALLS and the
//! management API calls before it dispatches a request. A reading, not a copy: it is called
//! with the shared engine and resolves the owner's own live records, so there is no second
//! inventory of which stages are deployed for a later create, delete or deployment release
//! to leave stale.
//!
//! LOCK ORDER: a caller releases this cell's own lock before it calls the reading
//! ([`ConnectionsEndpointAuthority::installed`] hands the function out by value for exactly
//! that reason), and the reading takes the owner's store itself — so nothing holds one
//! service's store while the other's is taken.

use std::any::Any;

/// What an endpoint addressing resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointAuthority {
    /// The invoke path names a stage an api of this emulator has deployed, so the request
    /// is the caller's to make and is served.
    Deployed,
    /// It names a stage no api of this emulator has deployed — a stage name that was never
    /// created, one whose stage holds no deployment, or one of an api this emulator does not
    /// serve — so the endpoint is not the caller's to invoke.
    Undeployed,
    /// Nothing that owns `execute-api` stages is mounted in this emulator, so no record says
    /// whether the path names a deployed stage and the addressing is not judged at all.
    Unjudged,
}

/// One reading of an endpoint addressing: the shared engine (as [`Any`], so this crate names
/// no engine type), the endpoint HOST the request was addressed at when it carries one, and
/// the invoke path PREFIX in front of the modelled `/@connections/{ConnectionId}` binding
/// (`"/dev"`, `"/v1/socket"`, or empty for the modelled path itself).
pub type EndpointReading = fn(&dyn Any, Option<&str>, &str) -> EndpointAuthority;

/// The installed reading.
///
/// Registered in the shared engine under THIS type, so the service that owns apis and stages
/// and the service that serves `@connections` reach one cell instead of each keeping a copy
/// of the other's records. The engine keys state by TYPE, so a cell of either service's own
/// would be invisible to the other.
#[derive(Debug, Default)]
pub struct ConnectionsEndpointAuthority {
    /// The reading the owner of apis and stages installed, or `None` while none is mounted.
    reading: Option<EndpointReading>,
}

impl ConnectionsEndpointAuthority {
    /// Install `reading` as the one judge of a `@connections` endpoint addressing.
    ///
    /// Called by the service that owns apis and stages when it is built, once per engine;
    /// installing again replaces the reading, which is the same function when the same
    /// service is mounted twice on one engine.
    pub fn install(&mut self, reading: EndpointReading) {
        self.reading = Some(reading);
    }

    /// The installed reading, by value so the caller can release this cell's lock before
    /// calling it.
    pub fn installed(&self) -> Option<EndpointReading> {
        self.reading
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reading that answers from the prefix alone, standing in for the owner's own
    /// resolution: what is pinned here is the cell, not any service's records.
    fn deployed_only_dev(_engine: &dyn Any, _host: Option<&str>, prefix: &str) -> EndpointAuthority {
        if prefix == "/dev" {
            EndpointAuthority::Deployed
        } else {
            EndpointAuthority::Undeployed
        }
    }

    #[test]
    fn a_cell_with_no_reading_judges_nothing() {
        let cell = ConnectionsEndpointAuthority::default();
        assert!(
            cell.installed().is_none(),
            "case \"no owner mounted\": expected no reading, got one"
        );
    }

    #[test]
    fn an_installed_reading_is_handed_back_and_judges_each_prefix() {
        let mut cell = ConnectionsEndpointAuthority::default();
        cell.install(deployed_only_dev);
        let reading = cell
            .installed()
            .expect("case \"an owner mounted\": the installed reading must be handed back");
        // The rows span what the reading generalises over: the prefix that names the
        // deployed stage, one that names another stage, and the modelled path's empty one.
        for (case, prefix, expected) in [
            ("the deployed stage", "/dev", EndpointAuthority::Deployed),
            ("another stage", "/test", EndpointAuthority::Undeployed),
            ("no prefix at all", "", EndpointAuthority::Undeployed),
        ] {
            let answered = reading(&() as &dyn Any, None, prefix);
            assert_eq!(
                answered, expected,
                "case {case:?}: expected {expected:?} for prefix {prefix:?}, got {answered:?}"
            );
        }
    }
}
