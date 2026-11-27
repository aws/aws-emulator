// SPDX-License-Identifier: CC0-1.0

//! The PORT a target group's protocol fixes.
//!
//! `Port` is "the port on which the targets receive traffic", bounded by the model at
//! `1..65535` — and its documentation closes with a rule the range cannot express: "If the
//! protocol is GENEVE, the supported port is 6081" (`CreateTargetGroupInput$Port`). The same
//! sentence is on the member that OVERRIDES that port for one target: "the port on which the
//! target is listening. If the target group protocol is GENEVE, the supported port is 6081"
//! (`TargetDescription$Port`).
//!
//! So a protocol the model documents ONE supported port for admits that port alone, wherever
//! a port is stated for a target group carrying it: a value inside the modelled range but
//! outside what the protocol supports is a pairing of members no target group can hold, so it
//! is refused rather than stored on a target group whose traffic could never reach its
//! targets.
//!
//! Read here ONCE, so the create that mints a target group and the registration that
//! overrides its port agree on which ports a protocol admits.

use super::Refusal;

/// The protocol a Gateway Load Balancer's target groups carry, from `ProtocolEnum`'s own
/// values: "For Gateway Load Balancers, the supported protocol is GENEVE."
pub const GENEVE_PROTOCOL: &str = "GENEVE";

/// The one port a GENEVE target group's targets receive traffic on, from
/// `CreateTargetGroupInput$Port`: "If the protocol is GENEVE, the supported port is 6081."
pub const GENEVE_PORT: i32 = 6081;

/// The ONE port a target group carrying `protocol` can receive traffic on, or `None` for a
/// protocol whose target groups may carry any port the model's range admits.
pub fn protocol_fixed_port(protocol: &str) -> Option<i32> {
    match protocol {
        GENEVE_PROTOCOL => Some(GENEVE_PORT),
        _ => None,
    }
}

/// Whether a target group carrying `protocol` can receive traffic on `port`, and — when it
/// cannot — the rule the request broke.
///
/// A request that states no protocol, or no port, is judged by no rule here: this is about
/// the PAIRING of the two values the request did carry. The refusal is the modelled "the
/// requested configuration is not valid" class, because each value is admitted on its own and
/// it is the combination that no target group can hold.
pub fn check_protocol_port(protocol: Option<&str>, port: Option<i32>) -> Result<(), Refusal> {
    let (Some(protocol), Some(port)) = (protocol, port) else {
        return Ok(());
    };
    let Some(supported) = protocol_fixed_port(protocol) else {
        return Ok(());
    };
    if port == supported {
        return Ok(());
    }
    Err(Refusal::Configuration(protocol_port_message(
        protocol, supported,
    )))
}

/// The sentence Elastic Load Balancing refuses such a pairing with: the port the protocol
/// supports, and the protocol that fixes it. Nothing of the request's own value appears in
/// it — what the caller has to fix is stated as the port it must use.
pub fn protocol_port_message(protocol: &str, port: i32) -> String {
    format!("Port must be {port} for {protocol} target groups.")
}
