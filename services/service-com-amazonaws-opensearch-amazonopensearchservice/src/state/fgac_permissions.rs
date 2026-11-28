// SPDX-License-Identifier: CC0-1.0

//! What a ROLE of a domain's security configuration permits, and what it lets the
//! principals it binds SEE — the one reading of the role documents the security plugin's
//! own API stored.
//!
//! A role mapping says WHICH principals hold a role ([`super::fgac::mapped_roles`]); the
//! role document says what holding it is worth. Three narrowings live in it, and all three
//! are read here so no path can apply one of them differently:
//!
//! * `cluster_permissions` — the cluster-level actions the role permits;
//! * `index_permissions` — for each set of `index_patterns`, the `allowed_actions` the role
//!   permits ON the indexes those patterns name. An action on an index no pattern of the
//!   role names is permitted by nothing, which is what makes a role an index boundary;
//! * `dls` and `fls` on one of those entries — WHICH documents of a named index the role
//!   may read, and which fields of each. Neither refuses a request: they narrow what it is
//!   answered with, which is what makes a hidden document indistinguishable from one the
//!   index does not hold.
//!
//! An `allowed_actions` entry may name an ACTION GROUP rather than an action
//! ([`ACTION_GROUPS`]), which is the form a client writes a role in.
//!
//! Nothing here reaches the clock or the network: every answer is a pure function of the
//! stored role documents and the request, so the same request decides the same way twice.

use std::collections::BTreeMap;

use ::serde_json::{json, Map, Value};
use ::service_common::iam_authorization::wildcard_match;

use super::{fgac, Domain};

/// The namespace an action ON AN INDEX is named under, as the security plugin's own action
/// names spell it (`indices:data/read/search`, `indices:admin/create`). It is what decides
/// which half of a role document an action is judged against: an action in this namespace
/// by `index_permissions`, every other by `cluster_permissions`.
const INDEX_ACTION_PREFIX: &str = "indices:";

/// The member of one `index_permissions` entry naming the indexes it is about.
const INDEX_PATTERNS: &str = "index_patterns";

/// The member naming the actions it permits on them.
const ALLOWED_ACTIONS: &str = "allowed_actions";

/// The member naming the cluster-level actions a role permits.
const CLUSTER_PERMISSIONS: &str = "cluster_permissions";

/// The member of one `index_permissions` entry carrying its document-level security query.
const DLS: &str = "dls";

/// The member carrying its field-level security rules.
const FLS: &str = "fls";

/// The prefix a field-level security rule carries to EXCLUDE the field it names rather
/// than to include it, as the plugin's own rule syntax spells it.
const EXCLUDED: char = '~';

/// The security plugin's DEFAULT ACTION GROUPS: the name an `allowed_actions` or
/// `cluster_permissions` entry may carry instead of an action, and the action patterns each
/// of them stands for.
///
/// Written down here because the data plane is recorded behaviour only — no model carries
/// it — and taken from the security plugin's own published default action groups, which is
/// what a client writing a role names. An entry naming none of these groups is read as an
/// action pattern in its own right, so a role that names an action directly, or a group
/// this table does not carry, is still evaluated against the action it is about.
const ACTION_GROUPS: &[(&str, &[&str])] = &[
    ("unlimited", &["*"]),
    ("indices_all", &["indices:*"]),
    ("cluster_all", &["cluster:*"]),
    ("get", &["indices:data/read/get*", "indices:data/read/mget*"]),
    (
        "read",
        &[
            "indices:data/read/*",
            "indices:admin/mappings/fields/get*",
            "indices:admin/resolve/index",
        ],
    ),
    (
        "search",
        &[
            "indices:data/read/search*",
            "indices:data/read/msearch*",
            "indices:data/read/scroll*",
            "indices:admin/resolve/index",
        ],
    ),
    (
        "write",
        &["indices:data/write/*", "indices:admin/mapping/put"],
    ),
    (
        "index",
        &[
            "indices:data/write/index*",
            "indices:data/write/update*",
            "indices:data/write/bulk*",
            "indices:admin/mapping/put",
        ],
    ),
    ("delete", &["indices:data/write/delete*"]),
    (
        "crud",
        &[
            "indices:data/read/*",
            "indices:data/write/*",
            "indices:admin/mapping/put",
        ],
    ),
    (
        "create_index",
        &["indices:admin/create", "indices:admin/mapping/put"],
    ),
    ("manage_aliases", &["indices:admin/aliases*"]),
    ("manage", &["indices:monitor/*", "indices:admin/*"]),
    ("cluster_monitor", &["cluster:monitor/*"]),
    (
        "cluster_composite_ops_ro",
        &[
            "indices:data/read/mget*",
            "indices:data/read/msearch*",
            "indices:data/read/mtv*",
            "indices:data/read/scroll*",
            "indices:data/read/search/template",
            "indices:admin/aliases/exists*",
            "indices:admin/aliases/get*",
        ],
    ),
    (
        "cluster_composite_ops",
        &[
            "indices:data/write/bulk*",
            "indices:data/write/reindex",
            "indices:data/read/mget*",
            "indices:data/read/msearch*",
            "indices:data/read/mtv*",
            "indices:data/read/scroll*",
            "indices:data/read/search/template",
            "indices:admin/aliases*",
        ],
    ),
    ("search_template", &["indices:data/read/search/template"]),
];

/// The default action groups a domain's security plugin PUBLISHES: each group's name and
/// the action patterns it stands for.
///
/// The reading half of [`ACTION_GROUPS`], for a caller that reports the catalogue rather
/// than resolving one entry of it — the comparison of two domains'
/// `security_action_groups` ([`crate::state::metadata_categories`]).
/// Every domain of this service publishes the same set, because the set is the plugin's
/// rather than a domain's.
#[must_use]
pub fn published_action_groups() -> BTreeMap<String, Vec<String>> {
    ACTION_GROUPS
        .iter()
        .map(|(group, actions)| {
            (
                (*group).to_string(),
                actions.iter().map(|action| (*action).to_string()).collect(),
            )
        })
        .collect()
}

/// Whether any of `roles` permits `action` on the index `index` names.
///
/// `index` is the index the request's own path addresses, and `None` for a request that
/// addresses the cluster rather than one index; an action in the index namespace that
/// names none is judged against every index, which is what a path like `/_search`
/// addresses.
#[must_use]
pub fn permitted(domain: &Domain, roles: &[String], action: &str, index: Option<&str>) -> bool {
    roles
        .iter()
        .any(|role| role_permits(domain, role, action, index))
}

/// Whether the ONE role `role` permits `action` on `index`.
///
/// Three readings, in order: the all-permitting role the master user holds
/// ([`fgac::ALL_ACCESS_ROLE`]) needs no document to be what it is; a role the configuration
/// holds a document for is judged by that document; and one of the plugin's PREDEFINED
/// roles ([`predefined_role`]) by the document it is.
///
/// A role that is none of those is one whose permissions cannot be READ here: the plugin
/// ships more roles than this surface states, and a client may have written one through a
/// channel this emulator does not hold. Nothing is known about what such a role grants, and
/// refusing on no evidence would deny what the cluster permits — the posture
/// [`super::ingest::iam_decision`] takes for a grant whose document cannot be read. So a
/// mapping to one is worth what a mapping was worth before any role document was consulted:
/// the principal is bound, and this surface narrows it no further.
fn role_permits(domain: &Domain, role: &str, action: &str, index: Option<&str>) -> bool {
    if role == fgac::ALL_ACCESS_ROLE {
        return true;
    }
    if let Some(stored) = domain.security_roles.get(role) {
        return document_permits(&stored.definition, action, index);
    }
    match predefined_role(role) {
        Some(document) => document_permits(&document, action, index),
        None => true,
    }
}

/// Whether the role DOCUMENT `definition` permits `action` on `index`: an action in the
/// index namespace against its `index_permissions`, and every other against its
/// `cluster_permissions`.
fn document_permits(definition: &Value, action: &str, index: Option<&str>) -> bool {
    if action.starts_with(INDEX_ACTION_PREFIX) {
        let named = index.unwrap_or("*");
        return index_permissions(definition).any(|permission| {
            names_index(permission, named)
                && listed(permission, ALLOWED_ACTIONS).any(|allowed| covers(allowed, action))
        });
    }
    listed(definition, CLUSTER_PERMISSIONS).any(|allowed| covers(allowed, action))
}

/// The document one of the security plugin's PREDEFINED roles IS — a role a domain holds
/// without any write having created it, so a role mapping may name it before the role API
/// has been used at all.
///
/// Written down here for the reason [`ACTION_GROUPS`] is, and taken from the same source:
/// the plugin's own published predefined roles. `all_access` is answered by
/// [`role_permits`] directly, since it permits everything; the read-only pair below are the
/// ones a client binds a reader to without writing a role of its own — `readall` grants
/// "search permissions for all indexes" plus the cluster-wide read-only composite
/// operations, and `readall_and_monitor` adds the cluster monitoring group to it.
fn predefined_role(role: &str) -> Option<Value> {
    let read_every_index = |cluster: Value| {
        json!({
            CLUSTER_PERMISSIONS: cluster,
            "index_permissions": [{
                INDEX_PATTERNS: ["*"],
                ALLOWED_ACTIONS: ["read"],
            }],
        })
    };
    match role {
        "readall" => Some(read_every_index(json!(["cluster_composite_ops_ro"]))),
        "readall_and_monitor" => Some(read_every_index(json!([
            "cluster_monitor",
            "cluster_composite_ops_ro"
        ]))),
        _ => None,
    }
}

/// Whether one `index_permissions` entry is about the index `named`.
fn names_index(permission: &Value, named: &str) -> bool {
    listed(permission, INDEX_PATTERNS).any(|pattern| wildcard_match(pattern, named, true))
}

/// Whether the `allowed_actions` entry `allowed` covers `action`: the action itself, a
/// wildcard pattern over it, or an action group that holds it ([`ACTION_GROUPS`]).
fn covers(allowed: &str, action: &str) -> bool {
    if wildcard_match(allowed, action, true) {
        return true;
    }
    ACTION_GROUPS.iter().any(|(group, actions)| {
        *group == allowed
            && actions
                .iter()
                .any(|pattern| wildcard_match(pattern, action, true))
    })
}

/// The `index_permissions` entries one role document carries.
fn index_permissions(definition: &Value) -> impl Iterator<Item = &Value> {
    definition
        .get("index_permissions")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
}

/// The texts the list member `member` of `value` carries, and none for a member it does not
/// carry or carries as anything but a list of texts.
fn listed<'v>(value: &'v Value, member: &str) -> impl Iterator<Item = &'v str> {
    value
        .get(member)
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(Value::as_str)
}

/// What a principal's roles let it SEE of ONE index: which of that index's documents it may
/// read, and which fields of each.
///
/// Both lists are EMPTY where the roles impose no narrowing, which is the common case: a
/// role that names an index and imposes no `dls` lets every document of it through, and one
/// that imposes no `fls` lets every field through. A principal holding two roles over one
/// index therefore sees the UNION of what each lets it see, which is how the plugin
/// combines them — a document or a field one role would hide is still visible when another
/// role the same principal holds does not hide it.
#[derive(Debug, Clone, Default)]
pub struct Visibility {
    /// The document-level security queries in force, as the query-DSL documents the roles
    /// carry: a document is visible when ANY of them selects it, and every document is
    /// visible when there are none.
    pub documents: Vec<Value>,
    /// The field-level security rules in force, as the roles carry them: a bare field name
    /// INCLUDES that field and a `~`-prefixed one EXCLUDES it ([`visible_source`]).
    pub fields: Vec<String>,
}

impl Visibility {
    /// Whether nothing is hidden at all, so a document can be answered exactly as it is
    /// held.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.documents.is_empty() && self.fields.is_empty()
    }

    /// `source` with every field these rules hide taken out ([`visible_source`]).
    #[must_use]
    pub fn visible(&self, source: &Value) -> Value {
        visible_source(&self.fields, source)
    }
}

/// What the roles `roles` let a principal see of the index `index`.
#[must_use]
pub fn visibility(domain: &Domain, roles: &[String], index: &str) -> Visibility {
    let mut documents: Vec<Value> = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    // A role that names the index and imposes no narrowing of its own OPENS it: the union
    // of what the principal's roles allow is what it sees.
    let mut every_document = false;
    let mut every_field = false;
    for role in roles {
        if role == fgac::ALL_ACCESS_ROLE {
            every_document = true;
            every_field = true;
            continue;
        }
        let Some(stored) = domain.security_roles.get(role) else {
            continue;
        };
        for permission in index_permissions(&stored.definition) {
            if !names_index(permission, index) {
                continue;
            }
            match dls_query(permission) {
                Some(query) => documents.push(query),
                None => every_document = true,
            }
            let rules: Vec<String> = listed(permission, FLS).map(str::to_string).collect();
            if rules.is_empty() {
                every_field = true;
            } else {
                for rule in rules {
                    if !fields.contains(&rule) {
                        fields.push(rule);
                    }
                }
            }
        }
    }
    if every_document {
        documents.clear();
    }
    if every_field {
        fields.clear();
    }
    Visibility { documents, fields }
}

/// The document-level security query one `index_permissions` entry imposes, or `None` where
/// it imposes none.
///
/// The member travels as the query-DSL document itself or as the TEXT of one — a client
/// writes it either way — so both are read, and a text that is not a JSON object imposes
/// nothing rather than hiding everything.
fn dls_query(permission: &Value) -> Option<Value> {
    match permission.get(DLS) {
        Some(Value::Object(query)) => Some(Value::Object(query.clone())),
        Some(Value::String(text)) if !text.trim().is_empty() => {
            ::serde_json::from_str::<Value>(text)
                .ok()
                .filter(Value::is_object)
        }
        _ => None,
    }
}

/// `source` with every field the field-level security rules `rules` hide taken out.
///
/// The rules are the plugin's own: a bare field name INCLUDES that field, so a rule set
/// holding any include names the WHOLE of what is visible and everything else is hidden;
/// a `~`-prefixed name EXCLUDES the field it names and leaves the rest visible. Each name
/// is the field's dotted path and may carry the `*` wildcard, which is what lets one rule
/// name a family of fields.
///
/// A rule set that is empty hides nothing, so the document is answered exactly as it is
/// held.
#[must_use]
pub fn visible_source(rules: &[String], source: &Value) -> Value {
    if rules.is_empty() {
        return source.clone();
    }
    let (excluded, included): (Vec<&String>, Vec<&String>) =
        rules.iter().partition(|rule| rule.starts_with(EXCLUDED));
    let excluded: Vec<&str> = excluded
        .iter()
        .map(|rule| rule.trim_start_matches(EXCLUDED))
        .collect();
    let included: Vec<&str> = included.iter().map(|rule| rule.as_str()).collect();
    visible_members(source, "", &included, &excluded)
}

/// `value` with the members the rules hide taken out, where `prefix` is the dotted path
/// `value` itself sits at.
///
/// Walks only objects: a value that is not one is a field's own value and is answered
/// whole or not at all, which is the decision its own path already made.
fn visible_members(
    value: &Value,
    prefix: &str,
    included: &[&str],
    excluded: &[&str],
) -> Value {
    let Value::Object(members) = value else {
        return value.clone();
    };
    let mut visible = Map::new();
    for (name, held) in members {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        if excluded
            .iter()
            .any(|pattern| wildcard_match(pattern, &path, true))
        {
            continue;
        }
        if included.is_empty() {
            visible.insert(name.clone(), visible_members(held, &path, included, excluded));
            continue;
        }
        if included
            .iter()
            .any(|pattern| wildcard_match(pattern, &path, true))
        {
            visible.insert(name.clone(), held.clone());
            continue;
        }
        // An include naming something UNDER this member keeps the member, narrowed to what
        // it names.
        let under = format!("{path}.");
        if included.iter().any(|pattern| pattern.starts_with(&under)) {
            visible.insert(name.clone(), visible_members(held, &path, included, excluded));
        }
    }
    Value::Object(visible)
}
