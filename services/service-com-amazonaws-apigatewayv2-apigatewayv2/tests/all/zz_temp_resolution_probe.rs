// SPDX-License-Identifier: CC0-1.0

//! Is the address an HTTP api publishes dialable AS GIVEN, and is the api served under it? One
//! probe of the published value, over a real connection to the socket that answers there.
//!
//! The reachability half of the rule `published_endpoint_documented_host` states the form of:
//!
//! > The value an api publishes is one a client DIALS, so its authority is an ADDRESS this
//! > emulator answers on — the `host:port` the core's listener landed on, which needs no
//! > resolver and can lead nowhere but this machine — and the api's data plane ANSWERS under
//! > the path published beneath it: a request naming that authority, with the api's path and
//! > then the stage name and route path appended as `Api$ApiEndpoint` documents ("the stage
//! > name is typically appended to this URI to form a complete path to a deployed API stage"),
//! > is answered by the api its path names.
//!
//! The rows are `published_endpoint_documented_host`'s — the Region the api was created in and
//! the operation that minted it — plus, per row, the two forms a client builds the invoke URL
//! in (the published value on its own, which addresses the stage that serves the api's root,
//! and that value with a stage name appended) and the twin api published beside it, which
//! shows the path and not the port is what says which api answers.
//!
//! The connection is made to the address the published value names, and the request NAMES that
//! same authority: the address is this emulator's and the path is the api's.

use crate::published_endpoint_documented_host::{
    emulator_served_on_a_port, host_of, minted, published_parts, rows, NAMED_STAGE, ROUTE_PATH,
    SERVED, UPSTREAM,
};
use crate::published_invoke_socket_front_door::{dialable_port, request};

/// RULE: the published authority needs no resolution and leads back to this process, and the
/// api is served under the path published beneath it — the route answers a request that names
/// that authority with the api's path and the stage path appended, and the twin api published
/// on the same socket answers under its own path alone.
#[tokio::test(flavor = "multi_thread")]
async fn the_published_address_is_dialable_as_given_and_the_api_is_served_under_it() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator_served_on_a_port();
        let api = minted(&emulator, &row).await;
        let twin = minted(&emulator, &row).await;
        let (authority, path) = published_parts(&api.api_endpoint);
        let (twin_authority, twin_path) = published_parts(&twin.api_endpoint);

        // The value a client is handed is dialled AS GIVEN: its host is an address literal, so
        // no resolver is consulted at all, and the address it names is on this machine.
        let address: std::net::IpAddr = host_of(&authority)
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse()
            .unwrap_or_else(|_| {
                panic!(
                    "case {case:?}: a published authority must be an address a client dials as \
                     given, got {:?}",
                    api.api_endpoint
                )
            });
        assert!(
            address.is_loopback(),
            "case {case:?}: the published address must never lead off this machine, got \
             {address:?} for {:?}",
            api.api_endpoint
        );

        // One socket serves the account's apis, so two published values name two paths on one
        // address: the PATH is what says which api answers.
        assert_eq!(
            authority, twin_authority,
            "case {case:?}: expected both apis published on one socket, got {authority:?} and \
             {twin_authority:?}"
        );
        assert_ne!(
            path, twin_path,
            "case {case:?}: expected each api published under its own path, got {path:?} for \
             both"
        );

        // The invoke URL a client builds from the published value: the stage name appended,
        // which for the stage that serves the api's root is no segment at all.
        let interface = format!("127.0.0.1:{}", dialable_port(case, &authority));
        for (form, stage_path) in [
            ("the published endpoint on its own", ROUTE_PATH.to_string()),
            (
                "the published endpoint with the stage name appended",
                format!("/{NAMED_STAGE}{ROUTE_PATH}"),
            ),
        ] {
            for (which, base, api_id) in [
                ("the api", &path, &api.api_id),
                ("its twin", &twin_path, &twin.api_id),
            ] {
                let target = format!("{base}{stage_path}");
                let answered = request(case, &interface, "GET", &target, &authority, &[]).await;
                assert_eq!(
                    answered.status, SERVED,
                    "case {case:?}: expected {which} ({api_id}) served at {form} ({target}) \
                     under {authority:?}, got {answered:?}"
                );
                assert_eq!(
                    answered.field("location"),
                    Some(UPSTREAM),
                    "case {case:?}: expected the request sent to the upstream {which}'s route \
                     names at {form} ({target}), got {answered:?}"
                );
            }
        }
    }
}
