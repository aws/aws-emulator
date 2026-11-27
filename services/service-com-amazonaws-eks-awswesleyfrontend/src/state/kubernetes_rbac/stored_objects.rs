// SPDX-License-Identifier: CC0-1.0

//! The RBAC objects a caller creates through the API, and the decision the ones this
//! cluster holds make about a request.
//!
//! Every write here is bounded before anything is stored — the objects one scope holds
//! ([`super::MAX_RBAC_OBJECTS_PER_SCOPE`]), the rules one role carries
//! ([`super::MAX_RULES_PER_ROLE`]), the subjects one binding carries
//! ([`super::MAX_SUBJECTS_PER_BINDING`]) and the entries one rule's own lists carry
//! ([`super::MAX_ENTRIES_PER_RULE`]) — which is also what makes the walk
//! [`ClusterObjects::rbac_admits`] does per request a bounded product of constants
//! rather than of whatever a caller sent.

use super::*;

use crate::state::kubernetes_resource_versions::admits_write;

/// The body a manifest of `resource` carries, or the `Status` the API server
/// refuses a manifest it cannot read as that resource with.
pub(super) fn body_from_manifest(
    resource: RbacResource,
    manifest: &::serde_json::Value,
) -> Result<RbacBody, ApiRefusal> {
    if resource.is_role() {
        return Ok(RbacBody::Rules(rules_from_manifest(manifest, resource)?));
    }
    // "roleRef can only reference a ClusterRole in the global namespace" or a Role
    // of the binding's own namespace: a binding that names no role binds nothing, so
    // the manifest is refused rather than stored.
    let reference = manifest.get("roleRef");
    let name = reference
        .and_then(|reference| reference.get("name"))
        .and_then(::serde_json::Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            ApiRefusal::invalid(format!(
                "{} is invalid: roleRef.name: Required value: name is required",
                resource.kind()
            ))
        })?;
    let kind = reference
        .and_then(|reference| reference.get("kind"))
        .and_then(::serde_json::Value::as_str)
        .filter(|kind| !kind.is_empty())
        .unwrap_or(RbacResource::ClusterRoles.kind());
    if kind != RbacResource::Roles.kind() && kind != RbacResource::ClusterRoles.kind() {
        return Err(ApiRefusal::invalid(format!(
            "{} is invalid: roleRef.kind: Unsupported value: \"{kind}\": supported values: \
             \"{}\", \"{}\"",
            resource.kind(),
            RbacResource::ClusterRoles.kind(),
            RbacResource::Roles.kind()
        )));
    }
    let api_group = reference
        .and_then(|reference| reference.get("apiGroup"))
        .and_then(::serde_json::Value::as_str)
        .unwrap_or(RBAC_API_GROUP)
        .to_string();
    let subjects = match reference {
        Some(_) => subjects_from_manifest(manifest, resource)?,
        None => Vec::new(),
    };
    Ok(RbacBody::Binding {
        role_ref: RoleRef {
            api_group,
            kind: kind.to_string(),
            name: name.to_string(),
        },
        subjects,
    })
}

/// The `rules` a role's manifest carries, or the `Status` the API server refuses a
/// manifest past what one role may hold with.
///
/// A role is a caller-sized thing the store keeps AND the authorizer walks per
/// request, so both the number of rules and the length of each rule's own lists are
/// bounded before anything is stored ([`MAX_RULES_PER_ROLE`],
/// [`MAX_ENTRIES_PER_RULE`]): the decision one request costs is then a bounded product
/// rather than whatever a caller sent. The refusal is the validation `Status`
/// Kubernetes answers a list past a limit with.
pub(super) fn rules_from_manifest(
    manifest: &::serde_json::Value,
    resource: RbacResource,
) -> Result<Vec<PolicyRule>, ApiRefusal> {
    let declared = manifest
        .get("rules")
        .and_then(::serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if declared.len() > MAX_RULES_PER_ROLE {
        return Err(too_many_items(
            resource,
            "rules",
            declared.len(),
            MAX_RULES_PER_ROLE,
        ));
    }
    let mut rules = Vec::with_capacity(declared.len());
    for rule in declared {
        rules.push(PolicyRule {
            api_groups: bounded_string_list(rule.get("apiGroups"), resource, "apiGroups")?,
            resources: bounded_string_list(rule.get("resources"), resource, "resources")?,
            resource_names: bounded_string_list(
                rule.get("resourceNames"),
                resource,
                "resourceNames",
            )?,
            verbs: bounded_string_list(rule.get("verbs"), resource, "verbs")?,
            non_resource_urls: bounded_string_list(
                rule.get("nonResourceURLs"),
                resource,
                "nonResourceURLs",
            )?,
        });
    }
    Ok(rules)
}

/// The `subjects` a binding's manifest carries, bounded by
/// [`MAX_SUBJECTS_PER_BINDING`] for the reason [`rules_from_manifest`] gives.
pub(super) fn subjects_from_manifest(
    manifest: &::serde_json::Value,
    resource: RbacResource,
) -> Result<Vec<Subject>, ApiRefusal> {
    let declared = manifest
        .get("subjects")
        .and_then(::serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if declared.len() > MAX_SUBJECTS_PER_BINDING {
        return Err(too_many_items(
            resource,
            "subjects",
            declared.len(),
            MAX_SUBJECTS_PER_BINDING,
        ));
    }
    Ok(declared
        .iter()
        .filter_map(|subject| {
            let kind = subject.get("kind").and_then(::serde_json::Value::as_str)?;
            let name = subject.get("name").and_then(::serde_json::Value::as_str)?;
            Some(Subject {
                kind: kind.to_string(),
                name: name.to_string(),
                namespace: subject
                    .get("namespace")
                    .and_then(::serde_json::Value::as_str)
                    .map(str::to_string),
                api_group: subject
                    .get("apiGroup")
                    .and_then(::serde_json::Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect())
}

/// The `Status` a manifest whose list `member` is longer than `limit` is refused with,
/// in the API server's own validation wording.
pub(super) fn too_many_items(
    resource: RbacResource,
    member: &str,
    declared: usize,
    limit: usize,
) -> ApiRefusal {
    ApiRefusal::invalid(format!(
        "{} is invalid: {member}: Too many: {declared}: must have at most {limit} items",
        resource.kind()
    ))
}

/// [`string_list`], bounded by [`MAX_ENTRIES_PER_RULE`].
pub(super) fn bounded_string_list(
    value: Option<&::serde_json::Value>,
    resource: RbacResource,
    member: &str,
) -> Result<Vec<String>, ApiRefusal> {
    let declared = value
        .and_then(::serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    if declared > MAX_ENTRIES_PER_RULE {
        return Err(too_many_items(
            resource,
            member,
            declared,
            MAX_ENTRIES_PER_RULE,
        ));
    }
    Ok(string_list(value))
}

/// The strings of a JSON array member, and none for a member that is absent or is
/// not an array of strings.
pub(super) fn string_list(value: Option<&::serde_json::Value>) -> Vec<String> {
    value
        .and_then(::serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(::serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

impl ClusterObjects {
    /// The map an object of `resource` in `namespace` is held in, or the `Status` a
    /// request naming a namespace this cluster does not hold is answered with.
    pub(super) fn rbac_scope(
        &self,
        resource: RbacResource,
        namespace: Option<&str>,
    ) -> Result<&RbacScope, ApiRefusal> {
        match (resource.is_namespaced(), namespace) {
            (true, Some(namespace)) => Ok(&self.namespace(namespace)?.rbac),
            _ => Ok(&self.cluster_rbac),
        }
    }

    /// [`Self::rbac_scope`], for a write.
    pub(super) fn rbac_scope_mut(
        &mut self,
        resource: RbacResource,
        namespace: Option<&str>,
    ) -> Result<&mut RbacScope, ApiRefusal> {
        match (resource.is_namespaced(), namespace) {
            (true, Some(namespace)) => {
                // Resolved first: an object cannot be created in a namespace that
                // does not exist.
                self.namespace(namespace)?;
                Ok(&mut self
                    .namespaces
                    .get_mut(namespace)
                    .ok_or_else(|| ApiRefusal::not_found("namespaces", namespace))?
                    .rbac)
            }
            _ => Ok(&mut self.cluster_rbac),
        }
    }

    /// `POST` of an RBAC object: store what the manifest names and answer it back.
    pub fn create_rbac_object(
        &mut self,
        resource: RbacResource,
        namespace: Option<&str>,
        manifest: &::serde_json::Value,
        now: SystemTime,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let name = manifest_name(manifest, resource.kind())?;
        let body = body_from_manifest(resource, manifest)?;
        let resource_version = self.versions.next();
        // A namespaced object is created in a namespace that takes new content: one a
        // delete has reached takes none ([`ClusterObjects::namespace_for_create`]).
        if let Some(namespace) = namespace.filter(|_| resource.is_namespaced()) {
            self.namespace_for_create(namespace, resource.resource(), &name)?;
        }
        let scope = self.rbac_scope_mut(resource, namespace)?;
        if scope.contains_key(&(resource, name.clone())) {
            return Err(ApiRefusal::already_exists(resource.resource(), &name));
        }
        if scope
            .keys()
            .filter(|(held, _)| *held == resource)
            .count()
            >= MAX_RBAC_OBJECTS_PER_SCOPE
        {
            return Err(ApiRefusal::too_many(
                resource.resource(),
                MAX_RBAC_OBJECTS_PER_SCOPE,
            ));
        }
        scope.insert(
            (resource, name.clone()),
            RbacObject {
                uid: new_object_uid(),
                created_at: now,
                resource_version,
                body,
            },
        );
        self.rbac_object_json(resource, namespace, &name)
    }

    /// `PUT` of an RBAC object: replace what the object holds, keeping the identity
    /// the create minted for it, which is what an update of an object does.
    ///
    /// A manifest carrying a `metadata.resourceVersion` asks for the write to be
    /// made only while the object still stands at that revision
    /// ([`super::super::kubernetes_resource_versions`]), so one carrying a revision
    /// the object has moved on from is refused and the rules it holds are left as
    /// they were.
    pub fn replace_rbac_object(
        &mut self,
        resource: RbacResource,
        namespace: Option<&str>,
        name: &str,
        manifest: &::serde_json::Value,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let body = body_from_manifest(resource, manifest)?;
        let resource_version = self.versions.next();
        let scope = self.rbac_scope_mut(resource, namespace)?;
        let held = scope
            .get_mut(&(resource, name.to_string()))
            .ok_or_else(|| ApiRefusal::not_found(resource.resource(), name))?;
        admits_write(
            resource.resource(),
            Some(RBAC_API_GROUP),
            name,
            held.resource_version,
            manifest,
        )?;
        held.body = body;
        held.resource_version = resource_version;
        self.rbac_object_json(resource, namespace, name)
    }

    /// `GET` of one RBAC object.
    pub fn rbac_object_json(
        &self,
        resource: RbacResource,
        namespace: Option<&str>,
        name: &str,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let held = self
            .rbac_scope(resource, namespace)?
            .get(&(resource, name.to_string()))
            .ok_or_else(|| ApiRefusal::not_found(resource.resource(), name))?;
        Ok(held.to_json(
            resource,
            name,
            resource.is_namespaced().then_some(namespace).flatten(),
        ))
    }

    /// `GET` of the collection, as the list a client reads.
    pub fn rbac_list_json(
        &self,
        resource: RbacResource,
        namespace: Option<&str>,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let scope = self.rbac_scope(resource, namespace)?;
        let items: Vec<::serde_json::Value> = scope
            .iter()
            .filter(|((held, _), _)| *held == resource)
            .map(|((_, name), object)| {
                object.to_json(
                    resource,
                    name,
                    resource.is_namespaced().then_some(namespace).flatten(),
                )
            })
            .collect();
        Ok(::serde_json::json!({
            "kind": resource.list_kind(),
            "apiVersion": RBAC_API_VERSION,
            "metadata": {},
            "items": items,
        }))
    }

    /// `DELETE` of one RBAC object, answered with the `Status` a delete is answered
    /// with — after which the rules it carried decide nothing.
    pub fn delete_rbac_object(
        &mut self,
        resource: RbacResource,
        namespace: Option<&str>,
        name: &str,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let scope = self.rbac_scope_mut(resource, namespace)?;
        scope
            .remove(&(resource, name.to_string()))
            .ok_or_else(|| ApiRefusal::not_found(resource.resource(), name))?;
        Ok(deleted_status_json(
            resource.resource(),
            name,
            Some(RBAC_API_GROUP),
        ))
    }

    /// Whether the RBAC objects this cluster holds admit `attributes` for
    /// `identity`.
    ///
    /// A `ClusterRoleBinding` binds its role over the whole cluster, so it admits a
    /// request in any namespace and a request that addresses none; a `RoleBinding`
    /// admits only a request in its own namespace, whether it binds a `Role` of that
    /// namespace or a `ClusterRole`.
    #[must_use]
    pub fn rbac_admits(
        &self,
        identity: &SubjectIdentity,
        attributes: &RequestAttributes,
    ) -> bool {
        let cluster_wide = self.bindings_admit(
            &self.cluster_rbac,
            RbacResource::ClusterRoleBindings,
            None,
            identity,
            attributes,
        );
        if cluster_wide {
            return true;
        }
        if attributes.namespace.is_empty() {
            return false;
        }
        let Some(namespace) = self.namespaces.get(&attributes.namespace) else {
            return false;
        };
        self.bindings_admit(
            &namespace.rbac,
            RbacResource::RoleBindings,
            Some(&attributes.namespace),
            identity,
            attributes,
        )
    }

    /// Whether any binding of `resource` in `scope` binds `identity` to a role whose
    /// rules admit `attributes`.
    pub(super) fn bindings_admit(
        &self,
        scope: &RbacScope,
        resource: RbacResource,
        namespace: Option<&str>,
        identity: &SubjectIdentity,
        attributes: &RequestAttributes,
    ) -> bool {
        scope
            .iter()
            .filter(|((held, _), _)| *held == resource)
            .filter(|(_, binding)| binding.binds(identity))
            .any(|(_, binding)| self.role_ref_admits(binding, namespace, attributes))
    }

    /// Whether the role a binding's `roleRef` names admits `attributes`.
    pub(super) fn role_ref_admits(
        &self,
        binding: &RbacObject,
        namespace: Option<&str>,
        attributes: &RequestAttributes,
    ) -> bool {
        self.role_ref_rules(binding, namespace).admits(attributes)
    }

    /// The rules the role a binding's `roleRef` names carries, borrowed from
    /// wherever that role lives.
    ///
    /// The ONE resolution of a `roleRef`, so the decision taken on a request
    /// ([`Self::role_ref_admits`]) and the rules a review reports
    /// ([`effective_rules`]) read the same role.
    pub(super) fn role_ref_rules(
        &self,
        binding: &RbacObject,
        namespace: Option<&str>,
    ) -> RoleRules<'_> {
        let RbacBody::Binding { role_ref, .. } = &binding.body else {
            return RoleRules::None;
        };
        if role_ref.kind == RbacResource::Roles.kind() {
            // A `Role` is resolved in the namespace the binding itself is in, and a
            // binding whose `Role` the namespace does not hold grants nothing — which
            // is how RBAC reads a dangling reference.
            let referenced = namespace.and_then(|namespace| {
                self.namespaces.get(namespace).and_then(|held| {
                    held.rbac
                        .get(&(RbacResource::Roles, role_ref.name.clone()))
                })
            });
            return match referenced.map(|role| &role.body) {
                Some(RbacBody::Rules(rules)) => RoleRules::Stored(rules),
                _ => RoleRules::None,
            };
        }
        match self
            .cluster_rbac
            .get(&(RbacResource::ClusterRoles, role_ref.name.clone()))
            .map(|role| &role.body)
        {
            Some(RbacBody::Rules(rules)) => RoleRules::Stored(rules),
            // A `ClusterRole` no object of this cluster carries is still one of the
            // roles Kubernetes installs on every cluster when it is one of those;
            // anything else is a dangling reference, which grants nothing.
            _ => match published_cluster_role(&role_ref.name) {
                Some(rules) => RoleRules::Published(rules),
                None => RoleRules::None,
            },
        }
    }

    /// Add the rules the bindings this cluster holds put in force for `identity` in
    /// `namespace` to `into`: the roles its `ClusterRoleBinding`s bind it to, which
    /// reach every namespace, and then the ones the `RoleBinding`s of that namespace
    /// bind it to.
    pub(super) fn bound_rules(
        &self,
        identity: &SubjectIdentity,
        namespace: &str,
        into: &mut Vec<EffectiveRule>,
    ) {
        self.binding_rules(
            &self.cluster_rbac,
            RbacResource::ClusterRoleBindings,
            None,
            identity,
            into,
        );
        if let Some(held) = self.namespaces.get(namespace) {
            self.binding_rules(
                &held.rbac,
                RbacResource::RoleBindings,
                Some(namespace),
                identity,
                into,
            );
        }
    }

    /// Add the rules every binding of `resource` in `scope` that binds `identity`
    /// puts in force to `into` — the collecting form of [`Self::bindings_admit`].
    fn binding_rules(
        &self,
        scope: &RbacScope,
        resource: RbacResource,
        namespace: Option<&str>,
        identity: &SubjectIdentity,
        into: &mut Vec<EffectiveRule>,
    ) {
        for (_, binding) in scope
            .iter()
            .filter(|((held, _), _)| *held == resource)
            .filter(|(_, binding)| binding.binds(identity))
        {
            self.role_ref_rules(binding, namespace).collect_into(into);
        }
    }
}

/// The rules of a published cluster role a binding may name even though no object
/// of that name was created: the roles Kubernetes installs on every cluster.
pub(super) fn published_cluster_role(role_name: &str) -> Option<&'static [PublishedRule]> {
    match role_name {
        "cluster-admin" => Some(CLUSTER_ADMIN_RULES),
        "admin" => Some(ADMIN_RULES),
        "edit" => Some(EDIT_RULES),
        "view" => Some(VIEW_RULES),
        _ => None,
    }
}
