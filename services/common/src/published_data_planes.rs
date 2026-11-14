// SPDX-License-Identifier: CC0-1.0

//! WHICH service answers at an ADDRESS this emulator published for a resource's own data
//! plane, so an in-process client handed that address can be routed to it.
//!
//! A resource whose data plane speaks its own protocol is published on a listener the CORE
//! opened for it (`engine.listeners()`), and what the owning service reports for a client to
//! dial is that listener's `host:port`
//! ([`crate::data_plane_addressing::listener_authority`]) — an address on the machine this
//! emulator runs on. A client that resolves AWS's own hostnames can read the service off the
//! HOST of an endpoint (`s3.us-west-2.amazonaws.com` names `s3`); nobody can read it off an
//! address, and [`crate::data_plane_addressing::names_a_service`] answers `false` for every
//! one of them. So the service that OPENED the listener records it here, under the authority
//! it published, and whatever has to route a request to an address a caller was handed reads
//! it back.
//!
//! The reader today is a RUNNING Lambda function's own HTTP request: a handler reads a
//! resource's endpoint out of the control plane (a collection's `collectionEndpoint`, a
//! cluster's `Endpoint`) and dials it, and the host half of that seam has to know which
//! service's credential scope to sign the in-process request with — which is exactly what
//! this records. Nothing here is read by the front door, which resolves such a request by
//! the host the owning service's own `Service::can_handle` claims.
//!
//! Engine-scoped, like every other store two services on either side of a hand-off share:
//! the records live in the SAME shared engine as the services' own, so a listener one
//! service opened is the one another resolves, and two engines in one process share nothing.
//! An engine this emulator did not build vends no store, and then an address simply resolves
//! to nothing — the position every reader was in before a service recorded one.
//!
//! LOCK ORDER: these records are taken LAST. A publisher holds its own service's store while
//! it records an address ([`PublishedPlanes::publish`] from a create, and
//! [`PublishedPlanes::withdraw`] from the drop of the handle that kept the port open), and
//! nothing here takes any other lock, so this store is never held while another is taken.

use std::collections::BTreeMap;

/// The addresses this emulator has published for resource data planes, and which service
/// answers at each.
///
/// Held once per engine. The key is the `host:port` authority a client dials, lower-cased so
/// a client that wrote an IPv6 literal or a name in another case resolves the same record;
/// the value is the SigV4 signing name of the service that opened the listener, which is the
/// credential scope a request to it is signed with and the key this emulator's front door
/// routes one by.
#[derive(Debug, Default)]
pub struct PublishedDataPlanes {
    answering: BTreeMap<String, String>,
}

/// A handle on one engine's [`PublishedDataPlanes`].
///
/// Clone-cheap (an `Arc` inside) and `'static`, so a service can keep one beside the
/// listener handle it opened and a seam that outlives the request that built it — a running
/// function's own — can hold one too.
#[derive(Clone)]
pub struct PublishedPlanes(::engine::ServiceState<PublishedDataPlanes>);

/// Written by hand rather than derived: the state handle inside carries no `Debug`, and what
/// is diagnosable about this handle is how many addresses it holds — never a caller's
/// address list in a log line.
impl std::fmt::Debug for PublishedPlanes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublishedPlanes")
            .field("published", &self.0.lock().answering.len())
            .finish()
    }
}

impl PublishedPlanes {
    /// The records of the SAME shared engine the services' own stores live in, or `None` for
    /// an engine that vends none.
    #[must_use]
    pub fn of<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<Self> {
        (engine as &dyn ::std::any::Any)
            .downcast_ref::<::engine::InMemoryEngine>()
            .map(|engine| Self(engine.state::<PublishedDataPlanes>()))
    }

    /// Record that `signing_name`'s service answers at `authority` — the `host:port` of a
    /// listener it has just opened for one of its resources.
    ///
    /// Call it with the address the CORE reports for the listener
    /// ([`crate::data_plane_addressing::listener_authority`]), so what is recorded is the
    /// address a client dials. Recording the same authority again replaces the record, which
    /// is what a port the operating system has handed out a second time needs.
    pub fn publish(&self, authority: &str, signing_name: &str) {
        self.0
            .lock()
            .answering
            .insert(key(authority), signing_name.to_string());
    }

    /// Forget `authority`: the listener behind it is closed, so nothing answers there any
    /// more and a client dialling it must not be routed as though something did.
    pub fn withdraw(&self, authority: &str) {
        self.0.lock().answering.remove(&key(authority));
    }

    /// The signing name of the service answering at `authority`, or `None` for an address
    /// this emulator has published nothing at.
    #[must_use]
    pub fn answering_service(&self, authority: &str) -> Option<String> {
        self.0.lock().answering.get(&key(authority)).cloned()
    }
}

/// One authority as it is keyed: as the client addressed it, lower-cased — a `host:port`,
/// where the port is part of the key because one address per listener is the point.
fn key(authority: &str) -> String {
    authority.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an address resolves to the service that PUBLISHED it, until that service
    /// withdraws it; an address nothing published resolves to nothing.
    ///
    /// The rows span what the reading generalises over: the address shapes a core listener
    /// is published at (an IPv4 literal with a port, a bracketed IPv6 one, a name), the case
    /// a client writes an address in, two listeners of two services on one engine, and the
    /// addresses that resolve to nothing (another port of the same host, a host nothing was
    /// published at).
    #[test]
    fn an_address_resolves_to_the_service_that_published_it() {
        let engine = ::engine::InMemoryEngine::new();
        let planes = PublishedPlanes::of(&engine).expect("the engine vends the records");
        let published: &[(&str, &str)] = &[
            ("127.0.0.1:37333", "aoss"),
            ("[::1]:40127", "rds"),
            ("localhost:8443", "execute-api"),
        ];
        for (authority, signing_name) in published {
            planes.publish(authority, signing_name);
        }
        let rows: &[(&str, &str, Option<&str>)] = &[
            ("the collection's own listener", "127.0.0.1:37333", Some("aoss")),
            ("an IPv6 listener", "[::1]:40127", Some("rds")),
            ("a listener on a name", "localhost:8443", Some("execute-api")),
            ("the same address in another case", "LOCALHOST:8443", Some("execute-api")),
            ("another port of the same host", "127.0.0.1:37334", None),
            ("a host nothing was published at", "10.0.0.1:443", None),
        ];
        for (case, authority, expected) in rows {
            let answered = planes.answering_service(authority);
            assert_eq!(
                answered.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {answered:?}"
            );
        }

        // A handle built again over the same engine reads the same records, which is what
        // lets the service that published an address and the seam that resolves it be in
        // two different crates.
        let read_again = PublishedPlanes::of(&engine).expect("the engine vends the records");
        assert_eq!(
            read_again.answering_service("127.0.0.1:37333").as_deref(),
            Some("aoss"),
            "case \"a second handle on the same engine\": expected the published record"
        );

        // Withdrawn with the listener: the address resolves to nothing again.
        read_again.withdraw("127.0.0.1:37333");
        let answered = planes.answering_service("127.0.0.1:37333");
        assert_eq!(
            answered, None,
            "case \"an address whose listener is closed\": expected nothing, got {answered:?}"
        );
    }

    /// An engine this emulator did not build vends no records, so an address resolves to
    /// nothing rather than taking the caller down.
    #[test]
    fn an_engine_that_vends_no_records_publishes_nothing() {
        struct Elsewhere;
        assert!(
            PublishedPlanes::of(&Elsewhere).is_none(),
            "expected no records for an engine of another kind"
        );
    }
}
