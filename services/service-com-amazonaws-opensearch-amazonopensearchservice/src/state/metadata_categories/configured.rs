// SPDX-License-Identifier: CC0-1.0

//! What a domain HOLDS in one metadata category: the entries each category is read as, keyed
//! by what identifies an entry in it, out of the stored domain record and nothing else.
//!
//! One arm per category of [`super::SUPPORTED_CATEGORIES`], which is the set this reading and
//! that catalogue are kept in step by: a comparison names a category, this is where the two
//! sides of it come from, and [`super::category_difference`] is what counts them.

use super::*;

/// The configuration `domain` holds in `category`, keyed by what identifies an entry in that
/// category and carrying the content a comparison calls diverged when it differs.
///
/// A category this service holds no per-domain record for contributes no entry, which counts
/// as zero on both sides rather than as a difference.
#[must_use]
pub fn configured(domain: &Domain, category: &str) -> BTreeMap<String, Value> {
    match category {
        // The cluster's own settings, keyed by the SCOPE and the setting's flat dotted name:
        // the two scopes hold the same name independently, so the scope is part of what
        // identifies a setting.
        "cluster_settings" => domain
            .cluster_settings
            .persistent
            .iter()
            .map(|(name, value)| (format!("{SETTINGS_PERSISTENT}/{name}"), json!(value)))
            .chain(
                domain
                    .cluster_settings
                    .transient
                    .iter()
                    .map(|(name, value)| (format!("{SETTINGS_TRANSIENT}/{name}"), json!(value))),
            )
            .collect(),
        // A role mapping is identified by the role it binds principals to, and its content
        // is the three member lists the security API reports.
        "security_role_mappings" => domain
            .security_role_mappings
            .iter()
            .map(|(role, mapping)| {
                (
                    role.clone(),
                    json!({
                        "backend_roles": mapping.backend_roles,
                        "users": mapping.users,
                        "hosts": mapping.hosts,
                    }),
                )
            })
            .collect(),
        "ism_policies" => domain
            .ism_policies
            .iter()
            .map(|(id, policy)| (id.clone(), policy.definition.clone()))
            .collect(),
        // An internal user is identified by its name. Its CREDENTIAL is no part of what is
        // compared: the domain holds it only to match a presented one against
        // ([`crate::state::fgac::Secret`]), and no read path reports it.
        "security_internal_users" => domain
            .internal_users
            .iter()
            .map(|(name, user)| {
                (
                    name.clone(),
                    json!({
                        "backend_roles": user.backend_roles,
                        "opendistro_security_roles": user.security_roles,
                        "attributes": user.attributes,
                    }),
                )
            })
            .collect(),
        // An alias is identified by its own name and names the indexes it stands for, which
        // is what differs between two domains holding one alias over different indexes.
        "aliases" => {
            let mut aliases: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for index in domain.indexes.values() {
                for alias in index.aliases.keys() {
                    aliases
                        .entry(alias.clone())
                        .or_default()
                        .insert(index.index_name.clone());
                }
            }
            aliases
                .into_iter()
                .map(|(alias, indexes)| (alias, json!(indexes)))
                .collect()
        }
        // A monitor is identified by its NAME rather than by the id the plugin minted for
        // it: two clusters configured alike hold the same monitor under two ids, and what is
        // being compared is the configuration. A monitor whose document names none is
        // identified by that id, which is all it has.
        "alerting_monitors" => domain
            .alerting
            .monitors
            .values()
            .map(|monitor| {
                let named = monitor
                    .definition
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(&monitor.id);
                (named.to_string(), monitor.definition.clone())
            })
            .collect(),
        "templates" => domain
            .index_templates
            .iter()
            .map(|(name, template)| (name.clone(), template.definition.clone()))
            .collect(),
        "component_templates" => domain
            .component_templates
            .iter()
            .map(|(name, template)| (name.clone(), template.definition.clone()))
            .collect(),
        "security_roles" => domain
            .security_roles
            .iter()
            .map(|(name, role)| (name.clone(), role.definition.clone()))
            .collect(),
        // The action groups a domain's security plugin publishes, which every domain of this
        // service publishes alike — so two domains differ in this category only when the
        // published set itself differs.
        "security_action_groups" => fgac_permissions::published_action_groups()
            .into_iter()
            .map(|(group, actions)| (group, json!(actions)))
            .collect(),
        "ingest_pipelines" => domain
            .ingest_pipelines
            .iter()
            .map(|(id, pipeline)| (id.clone(), pipeline.definition.clone()))
            .collect(),
        // Unreachable: every category reaching here is one of [`SUPPORTED_CATEGORIES`].
        _ => BTreeMap::new(),
    }
}
