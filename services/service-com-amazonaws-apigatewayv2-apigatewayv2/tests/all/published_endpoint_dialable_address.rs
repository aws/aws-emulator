// SPDX-License-Identifier: CC0-1.0

//! Is an api's DOCUMENTED `execute-api` host answered for at the address it published? One probe
//! per row, over a real connection to the socket that answers there.
//!
//! The other half of the rule `published_endpoint_documented_host` states the form of — that
//! half holds what an api PUBLISHES and `zz_temp_resolution_probe` that the published value is
//! dialled as given; this one holds what the same address answers for a client that reaches it
//! under AWS's own name:
//!
//! > An api's documented `execute-api` host — "The URI of the API, of the form
//! > `{api-id}.execute-api.{region}.amazonaws.com`" (`Api$ApiEndpoint`), whose three leading
//! > labels are the api's OWN id, the literal `execute-api` and the api's OWN Region — is
//! > answered for by the api's data plane at every address this emulator serves that plane on,
//! > the address the api published among them: a request naming it, with the stage name and
//! > route path appended, is served by the api its leading label names, whichever api of the
//! > account the address was published for.
//!
//! The rows are `published_endpoint_documented_host`'s — the Region the api was created in and
//! the operation that minted it — plus, per row, the stage the invoke path names (the reserved
//! `$default` stage that serves the api's root and a named one) and the twin api published
//! beside it, which shows the HOST and not the address is what says which api answers here.
//!
//! The host forms this addressing admits and refuses at each front door are
//! `execute_api_documented_authority`'s; what the socket decides for a request arriving there is
//! `published_invoke_socket_front_door`'s.

use crate::published_endpoint_documented_host::{
    emulator_served_on_a_port, minted, published_parts, rows, NAMED_STAGE, ROUTE_PATH, SERVED,
    UPSTREAM,
};
use crate::published_invoke_socket_front_door::request;

/// The label every `execute-api` endpoint host carries behind the api's own id, and the zone AWS
/// publishes that endpoint in.
const ENDPOINT_LABEL: &str = "execute-api";
const AWS_ZONE: &str = "amazonaws.com";

/// RULE: the api's documented `{api-id}.execute-api.{region}.amazonaws.com` host is answered for
/// by that api's own data plane at the address the api published, whichever Region the api was
/// created in, whichever operation minted it and whichever of its stages the path names.
#[tokio::test(flavor = "multi_thread")]
async fn the_documented_host_of_an_api_is_answered_for_at_the_published_address() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator_served_on_a_port();
        let api = minted(&emulator, &row).await;
        let twin = minted(&emulator, &row).await;
        // The address the request is MADE to is the one the api published; the name it carries
        // is the api's own, as a client that resolves AWS's hosts to this emulator sends it.
        let (dialled, _published) = published_parts(&api.api_endpoint);

        for (which, api_id) in [("the api", &api.api_id), ("its twin", &twin.api_id)] {
            let documented = format!("{api_id}.{ENDPOINT_LABEL}.{}.{AWS_ZONE}", row.region);
            for (form, path) in [
                ("the stage that serves the api's root", ROUTE_PATH.to_string()),
                (
                    "a named stage appended to the host",
                    format!("/{NAMED_STAGE}{ROUTE_PATH}"),
                ),
            ] {
                let answered = request(case, &dialled, "GET", &path, &documented, &[]).await;
                assert_eq!(
                    answered.status, SERVED,
                    "case {case:?}: expected {which} served under its documented host \
                     {documented:?} at {form} ({path}), got {answered:?}"
                );
                assert_eq!(
                    answered.field("location"),
                    Some(UPSTREAM),
                    "case {case:?}: expected the request sent to the upstream {which}'s route \
                     names under {documented:?} at {form} ({path}), got {answered:?}"
                );
            }
        }
    }
}
