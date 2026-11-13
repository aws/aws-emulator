// SPDX-License-Identifier: CC0-1.0

//! The preconditions Amazon MSK checks on the TARGET CLUSTER before it manages a
//! VPC connection — multi-VPC private connectivity — to that cluster.
//!
//! A VPC connection is a PrivateLink endpoint onto a cluster's brokers, so whether
//! one can be created at all is a question about the cluster, not about the
//! connection: the brokers have to be there to be reached (the cluster's STATE),
//! and they have to be willing to authenticate the client the connection carries
//! (the cluster's CLIENT AUTHENTICATION). Both sentences are the service's own,
//! shared by every operation that creates, deletes or rejects a connection — the
//! state sentence says so itself — so they are decided here once rather than
//! spelled per handler.
//!
//! Neither decision reads this registry's maps: each is a function of ONE cluster
//! record and the request, which is why this module sits beside the state rather
//! than inside it.

use super::{comma_or_list, Cluster, CLUSTER_STATES_WITH_BROKERS_IN_SERVICE};

/// The `Authentication` value a VPC connection authenticating with IAM access
/// control carries. The three published values are
/// [`super::VPC_CONNECTION_AUTHENTICATION_TYPES`], which is built from these.
pub const AUTHENTICATION_SASL_IAM: &str = "SASL_IAM";

/// The `Authentication` value for a connection authenticating with SASL/SCRAM.
pub const AUTHENTICATION_SASL_SCRAM: &str = "SASL_SCRAM";

/// The `Authentication` value for a connection authenticating with mutual TLS.
pub const AUTHENTICATION_TLS: &str = "TLS";

/// The `ClusterState` values Amazon MSK manages a cluster's VPC connections from,
/// as the refusal below lists them.
///
/// A VPC connection is placed onto the cluster's brokers, so the operations that
/// manage one are served only from a state in which the cluster HAS brokers that
/// are not being taken away from it — the service's one such set,
/// [`super::CLUSTER_STATES_WITH_BROKERS_IN_SERVICE`], which its SCRAM-secret
/// surface is served from too.
pub const CLUSTER_STATES_VPC_CONNECTIONS_ARE_MANAGED_FROM: [&str; 5] =
    CLUSTER_STATES_WITH_BROKERS_IN_SERVICE;

/// Whether a cluster reported in `state` serves an operation that creates, deletes
/// or rejects one of its VPC connections, per
/// [`CLUSTER_STATES_VPC_CONNECTIONS_ARE_MANAGED_FROM`]. One predicate, so those
/// operations cannot disagree about which states they are served from.
pub fn vpc_connections_managed_from_cluster_state(state: &str) -> bool {
    CLUSTER_STATES_VPC_CONNECTIONS_ARE_MANAGED_FROM.contains(&state)
}

/// The message Amazon MSK refuses a VPC-connection operation on a cluster whose
/// state does not serve it with.
///
/// Fixed text: the states come from
/// [`CLUSTER_STATES_VPC_CONNECTIONS_ARE_MANAGED_FROM`], so nothing of the request —
/// not the cluster ARN, not the cluster's own state — is interpolated, and the
/// sentence cannot drift from the set it names. It names all three operations
/// because the gate is one gate, whichever of them the caller reached.
pub fn invalid_cluster_state_for_vpc_connection_message() -> String {
    format!(
        "Invalid cluster state. To create, delete or reject a VPC connection, your cluster must \
         be in one of these states: {}.",
        comma_or_list(&CLUSTER_STATES_VPC_CONNECTIONS_ARE_MANAGED_FROM)
    )
}

/// The message Amazon MSK refuses a VPC connection whose `ClientSubnets` does not
/// span as many subnets as the TARGET CLUSTER's own client subnets with.
///
/// Fixed text in full: neither the counts, nor the subnet ids of either side, nor
/// the cluster reach the sentence. The member at fault travels in the refusal's
/// `invalidParameter`.
pub const CLIENT_SUBNET_COUNT_MESSAGE: &str =
    "Specify the same number of client subnets as cluster subnets.";

/// The subnets the CLUSTER itself is placed in — what a VPC connection's
/// `ClientSubnets` is sized against.
///
/// A provisioned cluster names them on its broker node group
/// (`BrokerNodeGroupInfo$ClientSubnets`); a serverless one names them on the VPC
/// configurations its clients reach it through (`VpcConfig$SubnetIds`), which is
/// the same fact for a cluster that has no broker node group of its own. One
/// derivation, so the gate below asks one question of either kind of cluster.
pub fn cluster_client_subnets(cluster: &Cluster) -> Vec<&str> {
    if !cluster.broker_node_group_info.client_subnets.is_empty() {
        return cluster
            .broker_node_group_info
            .client_subnets
            .iter()
            .map(String::as_str)
            .collect();
    }
    cluster
        .serverless
        .as_ref()
        .map(|serverless| {
            serverless
                .vpc_configs
                .iter()
                .flat_map(|config| config.subnet_ids.iter().map(String::as_str))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a VPC connection reaching `cluster` from `client_subnets` is placed
/// the way Amazon MSK places one: ONE elastic network interface per Availability
/// Zone the cluster has brokers in, so the connection spans exactly as many client
/// subnets as the cluster itself does ([`CLIENT_SUBNET_COUNT_MESSAGE`]).
///
/// Asked of the stored cluster and the request alone. A cluster whose record names
/// no client subnet at all ([`cluster_client_subnets`]) sizes nothing, so such a
/// cluster admits any non-empty list rather than refusing every one of them.
pub fn client_subnet_count_matches(cluster: &Cluster, client_subnets: &[String]) -> bool {
    let placed = cluster_client_subnets(cluster).len();
    placed == 0 || placed == client_subnets.len()
}

/// Whether the cluster turns the client-authentication scheme `authentication`
/// names on, so a VPC connection asking for that scheme has something to
/// authenticate against.
///
/// Multi-VPC private connectivity authenticates a client exactly as the cluster
/// does, and MSK turns a scheme on for VPC connectivity on top of the cluster's own
/// client authentication: a cluster that turns the scheme on in EITHER place
/// accepts clients authenticating that way, and one that turns it on in neither
/// accepts none — which is what the refusal below states. A value outside the
/// published set names no scheme at all, so no cluster has it enabled; the handler
/// refuses such a value as a bad parameter before asking this.
pub fn client_authentication_enabled(cluster: &Cluster, authentication: &str) -> bool {
    match authentication {
        AUTHENTICATION_SASL_IAM => {
            cluster.sasl_iam_enabled() || cluster.vpc_connectivity_sasl_iam_enabled()
        }
        AUTHENTICATION_SASL_SCRAM => {
            cluster.sasl_scram_enabled() || cluster.vpc_connectivity_sasl_scram_enabled()
        }
        AUTHENTICATION_TLS => {
            cluster.tls_client_authentication_enabled() || cluster.vpc_connectivity_tls_enabled()
        }
        _ => false,
    }
}

/// The scheme `authentication` names, written as Amazon MSK writes it in prose:
/// the published value with the separator between the SASL family and its
/// mechanism spelled as a slash (`SASL_IAM` -> `SASL/IAM`), which leaves a
/// single-word scheme (`TLS`) as it is.
pub fn client_authentication_label(authentication: &str) -> String {
    authentication.replace('_', "/")
}

/// The message Amazon MSK refuses a VPC connection asking for a client
/// authentication the target cluster does not turn on with — the same sentence the
/// SCRAM-secret surface answers for its own scheme
/// ([`super::SASL_SCRAM_NOT_ENABLED_MESSAGE`], which the tests below pin this
/// builder against so the two spellings of one AWS sentence cannot diverge), with
/// the scheme the request asked for in it: the condition is about the cluster's
/// configuration, so nothing else of the request reaches the text.
pub fn client_authentication_not_enabled_message(authentication: &str) -> String {
    format!(
        "{} client authentication must be enabled for the cluster.",
        client_authentication_label(authentication)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SCRAM surface holds this service's other spelling of the sentence this
    /// module builds, and two handlers answer with it, so the builder has to
    /// produce exactly that for the SASL/SCRAM scheme.
    #[test]
    fn the_scram_scheme_builds_the_sentence_the_scram_surface_answers() {
        let built = client_authentication_not_enabled_message(AUTHENTICATION_SASL_SCRAM);
        assert_eq!(
            built,
            super::super::SASL_SCRAM_NOT_ENABLED_MESSAGE,
            "case SASL_SCRAM: expected the sentence the SCRAM surface answers, got {built:?}"
        );
    }

    /// Every published scheme is labelled the way MSK writes it in prose, which is
    /// what the sentence above interpolates.
    #[test]
    fn every_published_scheme_is_labelled_as_msk_writes_it() {
        for (scheme, label) in [
            (AUTHENTICATION_SASL_IAM, "SASL/IAM"),
            (AUTHENTICATION_SASL_SCRAM, "SASL/SCRAM"),
            (AUTHENTICATION_TLS, "TLS"),
        ] {
            let built = client_authentication_label(scheme);
            assert_eq!(
                built, label,
                "case {scheme}: expected the label {label:?}, got {built:?}"
            );
        }
    }

    /// The state refusal names the set it is built from, in that order.
    #[test]
    fn the_state_refusal_lists_the_states_it_is_served_from() {
        let message = invalid_cluster_state_for_vpc_connection_message();
        assert_eq!(
            message,
            "Invalid cluster state. To create, delete or reject a VPC connection, your cluster \
             must be in one of these states: ACTIVE, UPDATING, MAINTENANCE, HEALING, or \
             REBOOTING_BROKER.",
            "expected the sentence Amazon MSK answers, got {message:?}"
        );
    }
}
