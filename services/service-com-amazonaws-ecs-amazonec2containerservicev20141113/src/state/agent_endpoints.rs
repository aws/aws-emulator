// SPDX-License-Identifier: CC0-1.0

//! The endpoints `DiscoverPollEndpoint` ANNOUNCES to the Amazon ECS container agent: where it
//! polls for the work assigned to its container instance, where it submits that instance's
//! telemetry, and where it polls for Service Connect configuration.
//!
//! All three are addresses the AGENT DIALS afterwards — `DiscoverPollEndpoint` "[r]eturns an
//! endpoint for the Amazon ECS agent to poll for updates", `endpoint` is "[t]he endpoint for
//! the Amazon ECS agent to poll", `telemetryEndpoint` "[t]he telemetry endpoint for the Amazon
//! ECS agent" and `serviceConnectEndpoint` "[t]he endpoint for the Amazon ECS agent to poll for
//! Service Connect configuration". So each has to be an address THIS emulator answers on: a
//! host of the real service's own endpoint family resolves to nothing on the machine this
//! emulator runs on or, worse, to genuine AWS infrastructure, and an agent handed one leaves the
//! emulated environment carrying the credentials it holds.
//!
//! The requirement is therefore RELATIONAL — whatever address this emulator is reached at is the
//! address it announces — so the value comes from
//! [`::smithy_server_runtime::OperationContext::endpoint`], "the base URL a later request
//! addresses… Callers send normally signed requests to it", rather than from a per-Region host
//! template. The Region a request was signed for does not enter it: one process answers on one
//! address, whichever Region a caller signs for, and this emulator serves every Region's Amazon
//! ECS surface there — including the agent's own calls.
//!
//! Amazon ECS answers the three on three different HOSTS, which one emulator endpoint cannot
//! do; what the agent needs of them is that they be three distinct addresses it can tell apart
//! and dial, so each is announced on the one address this process serves under a PATH of its
//! own. The base is the only authority a client on this machine can reach, and the path names
//! which of the three planes the address stands for, so no two members answer the same URL.
//!
//! The path costs no reachability: the agent-side protocols these addresses carry — the agent
//! communication and telemetry channels, and the Service Connect configuration poll — are not
//! among the surfaces this emulator serves at ANY address, so what a member can promise is the
//! authority a client can reach and which plane it stands for. The authority is the one this
//! process routes, and the path is where the plane is named; an agent that dials a plane this
//! emulator does not serve is answered by the router rather than by a host outside the machine.

/// The path segment [`agent_telemetry_endpoint`] announces its plane under, so the telemetry
/// address is not the poll address: Amazon ECS answers the two on different hosts, and this
/// emulator answers on one, so the plane is named in the path instead.
const TELEMETRY_PLANE_PATH: &str = "telemetry";

/// The path segment [`agent_service_connect_endpoint`] announces its plane under
/// ([`TELEMETRY_PLANE_PATH`]).
const SERVICE_CONNECT_PLANE_PATH: &str = "service-connect";

/// The endpoint this service announces to the agent for `plane`: the base URL this emulator is
/// served at, as the BASE URL every one of these members is documented and recorded as — an
/// address the agent appends its own path to, with the trailing `/` that makes it one — under
/// the path segment that distinguishes this plane from the others, if any.
///
/// `OperationContext::endpoint` reports that base as `scheme://host[:port]` and nothing else
/// (`EndpointUrl::parse` admits no path, query or fragment), so the announced URL carries the
/// scheme and authority this deployment is actually reached at and nothing is parsed out of it.
fn announced_agent_endpoint(emulator_endpoint: &str, plane: Option<&str>) -> String {
    let base = emulator_endpoint.trim_end_matches('/');
    match plane {
        Some(path) => format!("{base}/{path}/"),
        None => format!("{base}/"),
    }
}

/// "The endpoint for the Amazon ECS agent to poll", on the address this emulator is reached at
/// ([`announced_agent_endpoint`]) — the plane the agent polls for its work is the service's own
/// address, so it carries no further path.
pub fn agent_poll_endpoint(emulator_endpoint: &str) -> String {
    announced_agent_endpoint(emulator_endpoint, None)
}

/// "The telemetry endpoint for the Amazon ECS agent", on the address this emulator is reached at
/// ([`announced_agent_endpoint`]), under [`TELEMETRY_PLANE_PATH`].
pub fn agent_telemetry_endpoint(emulator_endpoint: &str) -> String {
    announced_agent_endpoint(emulator_endpoint, Some(TELEMETRY_PLANE_PATH))
}

/// "The endpoint for the Amazon ECS agent to poll for Service Connect configuration", on the
/// address this emulator is reached at ([`announced_agent_endpoint`]), under
/// [`SERVICE_CONNECT_PLANE_PATH`].
pub fn agent_service_connect_endpoint(emulator_endpoint: &str) -> String {
    announced_agent_endpoint(emulator_endpoint, Some(SERVICE_CONNECT_PLANE_PATH))
}

/// `ClientException$message` for a `containerInstance` this operation cannot resolve to an
/// instance of the cluster it was called for.
///
/// Entirely fixed text, and the agent plane's OWN sentence for the condition: the call comes
/// from the agent on the instance, not from a client holding a reference it can correct, so
/// Amazon ECS states that the resources the call is about could not be located rather than
/// naming the identifier back. The identifier-naming sentence the client-facing operations
/// answer that condition with ([`super::container_instance_not_found_message`]) is theirs:
/// `DeregisterContainerInstance` and `UpdateContainerAgent` are called ABOUT an instance a
/// client named, and this one is called BY it.
pub const AGENT_RESOURCES_NOT_LOCATED_MESSAGE: &str = "Could not locate resources";
