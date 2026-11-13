// SPDX-License-Identifier: CC0-1.0

//! Fine-grained access control on a domain of this service: the records the domain's own
//! security API holds, and the ONE decision every request that reaches that domain is
//! taken by.
//!
//! A domain created with `AdvancedSecurityOptions$Enabled` set has fine-grained access
//! control switched on, and on such a domain the cluster — not IAM alone — decides what a
//! principal may do: the master user the create named may do everything, and every other
//! principal may do only what a ROLE MAPPING binds it to. A principal no mapping binds is
//! refused, whatever its IAM policies allow, and the cluster states that refusal as a
//! `security_exception`.
//!
//! Both paths into a domain take the decision HERE so they cannot drift:
//!
//! * the domain's REST data plane ([`crate::service::domain_rest`]), which a client
//!   addresses at the endpoint `DescribeDomain` published, and
//! * the document write a producer outside this service performs ([`super::ingest`]) — a
//!   delivery stream whose destination is this domain, say — where the principal is the
//!   role that producer was configured with.
//!
//! Nothing here reaches the clock or the network: the decision is a pure function of the
//! stored domain and the principal, so the same request decides the same way twice.

use ::service_common::iam_authorization::wildcard_match;

use super::Domain;

/// The cluster action a request for the health of the cluster asks for, as the security
/// plugin's own action names spell it (`cluster:monitor/health`, the action the
/// `cluster_monitor` action group carries). A refusal names the action it refused.
pub const ACTION_CLUSTER_HEALTH: &str = "cluster:monitor/health";

/// The action a read of the cluster's own banner asks for.
pub const ACTION_CLUSTER_STATE: &str = "cluster:monitor/state";

/// The action a read of the cluster's nodes asks for.
pub const ACTION_NODES_INFO: &str = "cluster:monitor/nodes/info";

/// The action a read of the cluster's own settings asks for.
pub const ACTION_GET_CLUSTER_SETTINGS: &str = "cluster:monitor/settings";

/// The action updating those settings asks for.
pub const ACTION_UPDATE_CLUSTER_SETTINGS: &str = "cluster:admin/settings/update";

/// The action writing an ingest pipeline asks for.
pub const ACTION_PUT_PIPELINE: &str = "cluster:admin/ingest/pipeline/put";

/// The action reading one asks for.
pub const ACTION_GET_PIPELINE: &str = "cluster:admin/ingest/pipeline/get";

/// The action deleting one asks for.
pub const ACTION_DELETE_PIPELINE: &str = "cluster:admin/ingest/pipeline/delete";

/// The action running one over a document without storing it asks for.
pub const ACTION_SIMULATE_PIPELINE: &str = "cluster:admin/ingest/pipeline/simulate";

/// The action writing an index-management policy asks for.
pub const ACTION_PUT_ISM_POLICY: &str = "cluster:admin/opendistro/ism/policy/write";

/// The action reading one asks for.
pub const ACTION_GET_ISM_POLICY: &str = "cluster:admin/opendistro/ism/policy/get";

/// The action deleting one asks for.
pub const ACTION_DELETE_ISM_POLICY: &str = "cluster:admin/opendistro/ism/policy/delete";

/// The action writing an alerting MONITOR asks for, as the alerting plugin's own action
/// names spell it.
pub const ACTION_WRITE_MONITOR: &str = "cluster:admin/opendistro/alerting/monitor/write";

/// The action reading one asks for.
pub const ACTION_GET_MONITOR: &str = "cluster:admin/opendistro/alerting/monitor/get";

/// The action deleting one asks for.
pub const ACTION_DELETE_MONITOR: &str = "cluster:admin/opendistro/alerting/monitor/delete";

/// The action running one asks for.
pub const ACTION_EXECUTE_MONITOR: &str = "cluster:admin/opendistro/alerting/monitor/execute";

/// The action reading the alerts its triggers left asks for.
pub const ACTION_GET_ALERTS: &str = "cluster:admin/opendistro/alerting/alerts/get";

/// The action writing a legacy alerting DESTINATION asks for.
pub const ACTION_WRITE_DESTINATION: &str =
    "cluster:admin/opendistro/alerting/destination/write";

/// The action reading one asks for.
pub const ACTION_GET_DESTINATION: &str = "cluster:admin/opendistro/alerting/destination/get";

/// The action deleting one asks for.
pub const ACTION_DELETE_DESTINATION: &str =
    "cluster:admin/opendistro/alerting/destination/delete";

/// The action creating a NOTIFICATIONS channel configuration asks for, as the notifications
/// plugin's own action names spell it.
pub const ACTION_CREATE_NOTIFICATION_CONFIG: &str =
    "cluster:admin/opensearch/notifications/configs/create";

/// The action reading one asks for.
pub const ACTION_GET_NOTIFICATION_CONFIG: &str =
    "cluster:admin/opensearch/notifications/configs/get";

/// The action updating one asks for.
pub const ACTION_UPDATE_NOTIFICATION_CONFIG: &str =
    "cluster:admin/opensearch/notifications/configs/update";

/// The action deleting one asks for.
pub const ACTION_DELETE_NOTIFICATION_CONFIG: &str =
    "cluster:admin/opensearch/notifications/configs/delete";

/// The action a write of one document asks for.
pub const ACTION_INDEX_DOCUMENT: &str = "indices:data/write/index";

/// The action a BULK write asks for — the one action the bulk API itself is registered
/// under, beside the per-item action each of its items asks for.
pub const ACTION_BULK: &str = "indices:data/write/bulk";

/// The action adding or removing an alias asks for.
pub const ACTION_ALIASES: &str = "indices:admin/aliases";

/// The action reading an index's aliases asks for.
pub const ACTION_GET_ALIASES: &str = "indices:admin/aliases/get";

/// The action reading an index's own settings asks for.
pub const ACTION_GET_SETTINGS: &str = "indices:monitor/settings/get";

/// The action updating an index's settings asks for.
pub const ACTION_UPDATE_SETTINGS: &str = "indices:admin/settings/update";

/// The action reading an index's field mappings asks for.
pub const ACTION_GET_MAPPING: &str = "indices:admin/mappings/get";

/// The action adding to those mappings asks for.
pub const ACTION_PUT_MAPPING: &str = "indices:admin/mapping/put";

/// The action the index listing of the `_cat` APIs asks for — the index statistics those
/// rows report.
pub const ACTION_INDICES_STATS: &str = "indices:monitor/stats";

/// The action writing a component template asks for.
pub const ACTION_PUT_COMPONENT_TEMPLATE: &str = "cluster:admin/component_template/put";

/// The action reading one asks for.
pub const ACTION_GET_COMPONENT_TEMPLATE: &str = "cluster:admin/component_template/get";

/// The action a search asks for.
pub const ACTION_SEARCH: &str = "indices:data/read/search";

/// The action a refresh asks for — making the writes already taken visible to search.
pub const ACTION_REFRESH: &str = "indices:admin/refresh";

/// The action a count asks for.
pub const ACTION_COUNT: &str = "indices:data/read/count";

/// The action reporting how a text is analyzed asks for — the index-administration action
/// the engine's own analyze API is registered under.
pub const ACTION_ANALYZE: &str = "indices:admin/analyze";

/// The action a read of one document by its id asks for.
pub const ACTION_GET_DOCUMENT: &str = "indices:data/read/get";

/// The action deleting one document asks for.
pub const ACTION_DELETE_DOCUMENT: &str = "indices:data/write/delete";

/// The action deleting every document a query selects asks for — the action the engine's own
/// delete-by-query API is registered under.
pub const ACTION_DELETE_BY_QUERY: &str = "indices:data/write/delete/byquery";

/// The action creating an index asks for.
pub const ACTION_CREATE_INDEX: &str = "indices:admin/create";

/// The action deleting an index asks for.
pub const ACTION_DELETE_INDEX: &str = "indices:admin/delete";

/// The action reading an index's own settings and mappings asks for.
pub const ACTION_GET_INDEX: &str = "indices:admin/get";

/// The action reading an index's own settings asks for — the monitoring action the
/// engine's get-settings API is registered under.
pub const ACTION_GET_INDEX_SETTINGS: &str = "indices:monitor/settings/get";

/// The action updating those settings asks for.
pub const ACTION_UPDATE_INDEX_SETTINGS: &str = "indices:admin/settings/update";

/// The action writing an index template asks for.
pub const ACTION_PUT_TEMPLATE: &str = "indices:admin/index_template/put";

/// The action reading an index template asks for.
pub const ACTION_GET_TEMPLATE: &str = "indices:admin/index_template/get";

/// The action changing the security configuration itself asks for — the one only the
/// master user holds, which is why the security API is reachable by nobody else.
pub const ACTION_SECURITY_CONFIG: &str = "cluster:admin/security/config/update";

/// The role the cluster grants every permission, from the security plugin's predefined
/// roles: `all_access`. It is the role the control plane binds a domain's MASTER USER to,
/// which is why the master is reported holding it.
pub const ALL_ACCESS_ROLE: &str = "all_access";

/// One role of a domain's security configuration, as its security API holds it.
///
/// The definition is kept VERBATIM as the document that was written, because the API
/// reports back what it was given; what the emulator reads out of it is nothing — a role's
/// permissions bound what a MAPPED principal may do, which is a narrowing this emulator
/// does not model, so the record exists to be reported and to be named by a mapping.
#[derive(Debug, Clone)]
pub struct SecurityRole {
    /// The role's name — unique in the configuration, so the name IS the key.
    pub name: String,
    /// The role document as it was written.
    pub definition: ::serde_json::Value,
}

/// One role MAPPING of a domain's security configuration: which principals hold a role.
#[derive(Debug, Clone, Default)]
pub struct SecurityRoleMapping {
    /// The role this mapping binds principals to.
    pub role: String,
    /// `backend_roles` — the external identities the authentication backend asserts,
    /// which for a request signed with AWS credentials are the IAM identities the
    /// credential speaks for. Each entry may carry the `*` and `?` wildcards.
    pub backend_roles: Vec<String>,
    /// `users` — the subject names the mapping binds directly.
    pub users: Vec<String>,
    /// `hosts` — kept so the API reports back what it was given.
    pub hosts: Vec<String>,
}

/// Whether fine-grained access control is the authorization boundary on `domain`:
/// `AdvancedSecurityOptions$Enabled`. A domain that carries no such options, or carries
/// them switched off, is decided by IAM alone.#[must_use]
pub fn enforced(domain: &Domain) -> bool {
    domain
        .advanced_security_options
        .as_ref()
        .and_then(|options| options.enabled)
        == Some(true)
}

/// The identities a request signed as `principal_arn` is matched by: the principal itself,
/// and — for an assumed-role session — the IAM ROLE it is a session of, which is the
/// identity a `backend_roles` entry names.
///
/// A session ARN is `arn:<partition>:sts::<account>:assumed-role/<RoleName>/<SessionName>`
/// and the role it names is `arn:<partition>:iam::<account>:role/<RoleName>`, which is the
/// value a client puts in a role mapping because it is the value it created the role with.
#[must_use]
pub fn identities(principal_arn: &str) -> Vec<String> {
    let mut identities = vec![principal_arn.to_string()];
    if let Some(parsed) = ::service_common::arn::parse(principal_arn) {
        if parsed.vendor == "sts" {
            if let Some(role_name) = parsed
                .resource
                .strip_prefix("assumed-role/")
                .and_then(|rest| rest.split('/').next())
                .filter(|name| !name.is_empty())
            {
                identities.push(format!(
                    "arn:{}:iam::{}:role/{role_name}",
                    parsed.partition, parsed.account
                ));
            }
        }
    }
    identities
}

/// Whether `principal_arn` is the master user the domain was created with — the principal
/// fine-grained access control grants everything, and the only one the security API itself
/// is reachable by.
#[must_use]
pub fn is_master(domain: &Domain, principal_arn: &str) -> bool {
    let identities = identities(principal_arn);
    let named = |configured: &String| {
        identities
            .iter()
            .any(|identity| identity.eq_ignore_ascii_case(configured))
    };
    domain.master_user_arn.as_ref().is_some_and(named)
        || domain.master_user_name.as_ref().is_some_and(named)
}

/// The roles this domain's role mappings bind `principal_arn` to, in configuration order.
#[must_use]
pub fn mapped_roles(domain: &Domain, principal_arn: &str) -> Vec<String> {
    let identities = identities(principal_arn);
    domain
        .security_role_mappings
        .values()
        .filter(|mapping| {
            mapping
                .backend_roles
                .iter()
                .chain(mapping.users.iter())
                .any(|pattern| {
                    identities
                        .iter()
                        .any(|identity| wildcard_match(pattern, identity, true))
                })
        })
        .map(|mapping| mapping.role.clone())
        .collect()
}

/// The reason fine-grained access control refuses `principal_arn` the `action`, or `None`
/// when it permits it.
///
/// `None` for every domain the boundary is not switched on for, so a domain without
/// fine-grained access control is decided by IAM exactly as it was before.
#[must_use]
pub fn refusal(domain: &Domain, principal_arn: &str, action: &str) -> Option<String> {
    if !enforced(domain) || is_master(domain, principal_arn) {
        return None;
    }
    if !mapped_roles(domain, principal_arn).is_empty() {
        return None;
    }
    Some(no_permissions(principal_arn, action))
}

/// The reason only the master user may change the security configuration, or `None` when
/// `principal_arn` IS that master user.
#[must_use]
pub fn security_api_refusal(domain: &Domain, principal_arn: &str) -> Option<String> {
    (!is_master(domain, principal_arn))
        .then(|| no_permissions(principal_arn, ACTION_SECURITY_CONFIG))
}

/// How the security plugin states a request it does not permit: the action it refused and
/// the user it refused it for, with that user's own name and the backend roles it arrived
/// with. Every value comes from the request.
#[must_use]
pub fn no_permissions(principal_arn: &str, action: &str) -> String {
    format!(
        "no permissions for [{action}] and {}",
        user_description(principal_arn)
    )
}

/// How the security plugin DESCRIBES the user a request speaks for: its own name, the
/// backend roles it arrived with and the tenant it asked for.
///
/// One wording, because two answers carry it: the `security_exception` a refusal states
/// ([`no_permissions`]) and the `user` the plugin's own `authinfo` reports
/// ([`crate::service::domain_rest`]) — so what a caller is told it is cannot differ
/// between the two.
#[must_use]
pub fn user_description(principal_arn: &str) -> String {
    format!(
        "User [name={principal_arn}, backend_roles=[{}], requestedTenant=null]",
        backend_roles(principal_arn).join(", ")
    )
}

/// The backend roles a request signed as `principal_arn` arrives with: the identities the
/// authentication backend asserts for it BESIDE its own name ([`identities`]), which for
/// an AWS principal is the IAM role a session speaks for — the identity a role mapping is
/// written with.
#[must_use]
pub fn backend_roles(principal_arn: &str) -> Vec<String> {
    identities(principal_arn).into_iter().skip(1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a request signed as an assumed-role session is matched by its own ARN AND by
    /// the IAM role it is a session of, which is the identity a role mapping names.
    #[test]
    fn a_session_is_matched_by_the_role_it_is_a_session_of() {
        // (the case, the principal, the identities it is matched by)
        let rows: &[(&str, &str, &[&str])] = &[
            (
                "an assumed-role session",
                "arn:aws:sts::000000000000:assumed-role/firehose-delivery/session-1",
                &[
                    "arn:aws:sts::000000000000:assumed-role/firehose-delivery/session-1",
                    "arn:aws:iam::000000000000:role/firehose-delivery",
                ],
            ),
            (
                "an IAM role, as a producer writes as",
                "arn:aws:iam::000000000000:role/firehose-delivery",
                &["arn:aws:iam::000000000000:role/firehose-delivery"],
            ),
            (
                "an IAM user",
                "arn:aws:iam::000000000000:user/analyst",
                &["arn:aws:iam::000000000000:user/analyst"],
            ),
            ("a value that is no ARN", "master", &["master"]),
        ];
        for (case, principal, expected) in rows {
            let answered = identities(principal);
            assert_eq!(
                answered, *expected,
                "case {case:?}: expected {expected:?}, got {answered:?}"
            );
        }
    }

    /// RULE: the refusal names the action it refused and the user it refused it for, with
    /// the backend roles that user arrived with.
    #[test]
    fn the_refusal_names_the_action_and_the_user() {
        let message = no_permissions(
            "arn:aws:sts::000000000000:assumed-role/firehose-delivery/session-1",
            ACTION_INDEX_DOCUMENT,
        );
        for part in [
            "no permissions for [indices:data/write/index]",
            "name=arn:aws:sts::000000000000:assumed-role/firehose-delivery/session-1",
            "backend_roles=[arn:aws:iam::000000000000:role/firehose-delivery]",
            "requestedTenant=null",
        ] {
            assert!(
                message.contains(part),
                "case {part:?}: expected the refusal to carry it, got {message:?}"
            );
        }
    }

    /// RULE: the user the plugin describes carries the same name and backend roles a
    /// refusal names, so `authinfo` and a `security_exception` cannot disagree about who
    /// the caller is.
    #[test]
    fn the_user_description_is_the_one_a_refusal_names() {
        // (the case, the principal, the backend roles it arrives with)
        let rows: &[(&str, &str, &[&str])] = &[
            (
                "an assumed-role session",
                "arn:aws:sts::000000000000:assumed-role/search-reader/session-1",
                &["arn:aws:iam::000000000000:role/search-reader"],
            ),
            (
                "an IAM role",
                "arn:aws:iam::000000000000:role/search-reader",
                &[],
            ),
        ];
        for (case, principal, expected) in rows {
            let described = user_description(principal);
            assert!(
                no_permissions(principal, ACTION_SEARCH).contains(&described),
                "case {case:?}: expected the refusal to carry the description \
                 {described:?}"
            );
            let answered = backend_roles(principal);
            assert_eq!(
                answered, *expected,
                "case {case:?}: expected the backend roles {expected:?}, got {answered:?}"
            );
        }
    }
}
