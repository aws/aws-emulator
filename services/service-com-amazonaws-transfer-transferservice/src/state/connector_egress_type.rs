// SPDX-License-Identifier: CC0-1.0

//! WHICH egress a connector's configuration selects.
//!
//! `EgressConfig` "determines how traffic is routed from the connector to the SFTP
//! server", and `EgressType` is the "type of egress configuration for the connector":
//! `SERVICE_MANAGED` "uses Transfer Family managed NAT gateways", while `VPC_LATTICE`
//! "routes traffic through customer VPCs using VPC Lattice". So the type is not a member
//! a caller sets on its own — no write operation has one — it is READ OFF the egress
//! configuration the request carries, and `EgressType`'s modelled default
//! ([`DEFAULT_CONNECTOR_EGRESS_TYPE`]) is what a connector configured with none has.
//!
//! It is resolved in one place because everything else about a connector is stated
//! against it: whether a `Url` is required or must be null, whether an `EgressConfig` may
//! be carried at all ([`ConnectorConfig::cross_member_violation`]), and which egress
//! addresses the create assigns ([`service_managed_egress_ip_addresses`]). Reading it
//! from one function is what keeps the handler's checks and the stored record from
//! disagreeing.

use super::*;

/// The `EgressType` a connector created with `egress_config` has.
///
/// `ConnectorEgressConfig`'s only member is `VpcLattice`, so a request that carries an
/// egress configuration is a `VPC_LATTICE` connector and one that carries none takes
/// `EgressType`'s modelled default. Both the create handler's cross-member checks and
/// the stored record read the type from here, so the two cannot disagree.
pub fn connector_egress_type(egress_config: Option<&ConnectorEgressConfig>) -> &'static str {
    match egress_config {
        Some(_) => CONNECTOR_EGRESS_TYPE_VPC_LATTICE,
        None => DEFAULT_CONNECTOR_EGRESS_TYPE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the type is read off the configuration — a connector carrying an egress
    /// configuration routes through the customer's VPC, and one carrying none takes the
    /// member's documented default.
    #[test]
    fn the_egress_type_is_read_off_the_egress_configuration() {
        let cases: Vec<(&str, Option<ConnectorEgressConfig>, &str)> = vec![
            (
                "no egress configuration at all",
                None,
                CONNECTOR_EGRESS_TYPE_SERVICE_MANAGED,
            ),
            (
                "a VPC Lattice egress configuration",
                Some(ConnectorEgressConfig::VpcLattice(
                    ConnectorVpcLatticeEgressConfig {
                        resource_configuration_arn:
                            "arn:aws:vpc-lattice:us-east-1:000000000000:resourceconfiguration/\
                             rcfg-0123456789abcdef0"
                                .to_string(),
                        port_number: None,
                    },
                )),
                CONNECTOR_EGRESS_TYPE_VPC_LATTICE,
            ),
        ];

        for (case, egress_config, expected) in &cases {
            let resolved = connector_egress_type(egress_config.as_ref());
            assert_eq!(
                resolved, *expected,
                "case {case:?}: expected the EgressType {expected:?}, got {resolved:?}"
            );
        }
    }
}
