// SPDX-License-Identifier: CC0-1.0

//! The ConfigMaps one cluster's API server holds.
//!
//! A ConfigMap is how configuration reaches what runs on a cluster: a client
//! creates one through the published API server address and the workloads that
//! read it are configured by it. Amazon EKS delivers its own cluster
//! configuration this way too — Fargate log routing is a ConfigMap in the
//! `aws-observability` namespace, and a cluster authenticating through
//! `aws-auth` is a ConfigMap in `kube-system` — so the collection has to serve
//! the create, not only the reads: a `405` says the verb is unsupported by the
//! resource, which is a permanent answer a client gives up on.
//!
//! The objects live inside their namespace ([`super::kubernetes_objects`]), so
//! deleting the cluster takes them with it and two clusters never share one. The
//! create is the only write a request makes, and every answer is computed from
//! what that write stored.

use super::kubernetes_lists::{listed, ListRequest};
use super::kubernetes_metadata::{invalid_member, string_value_map, ObjectMetadata};
use super::kubernetes_objects::{
    deleted_status_json, manifest_name, new_object_uid, ApiRefusal, ClusterObjects,
    CORE_API_VERSION,
};
use super::kubernetes_resource_versions::admits_write;
use super::*;

/// How many ConfigMaps one namespace holds.
///
/// CHOSEN, not published: the Kubernetes API puts no limit on how many
/// ConfigMaps a namespace holds, and a caller can create them without end, so
/// the store needs a bound of its own. It is the bound this plane already puts
/// on the pods and the service accounts of one namespace, far above the handful
/// a cluster of this emulator is driven with.
pub const MAX_CONFIG_MAPS_PER_NAMESPACE: usize = 256;

/// The most bytes one ConfigMap's `data` and `binaryData` hold together.
///
/// PUBLISHED: "The data stored in a ConfigMap cannot exceed 1 MiB. If you need
/// to store settings that are larger than this limit, you may want to consider
/// mounting a volume or use a separate database or file service" (Kubernetes
/// ConfigMap concept documentation). Measured over the keys and the values as
/// STORED — a `binaryData` value by the bytes it decodes to rather than by the
/// base64 text that carries them — which is what the API server measures.
pub const MAX_CONFIG_MAP_BYTES: usize = 1024 * 1024;

/// The resource a ConfigMap is addressed and refused by.
pub const CONFIG_MAPS_RESOURCE: &str = "configmaps";

/// One ConfigMap: the configuration a client stored under a name, as it reads
/// back.
#[derive(Debug)]
pub struct ConfigMapObject {
    pub uid: String,
    pub created_at: SystemTime,
    /// The revision the write that last stored this ConfigMap was stamped with,
    /// which a client hands back to update it
    /// ([`super::kubernetes_resource_versions`]).
    pub resource_version: u64,
    /// The labels and annotations the manifest carried.
    pub metadata: ObjectMetadata,
    /// `data`: the UTF-8 configuration values, by key.
    pub data: BTreeMap<String, String>,
    /// `binaryData`: the values whose bytes are not UTF-8, by key, kept in the
    /// base64 the member carries them in.
    pub binary_data: BTreeMap<String, String>,
}

impl ConfigMapObject {
    /// This ConfigMap as the object a client reads: the keys it was created
    /// with, under the metadata the server minted for it.
    ///
    /// An empty `data` or `binaryData` is left out rather than reported as an
    /// empty object, which is how the API server reports a ConfigMap carrying
    /// none.
    #[must_use]
    pub fn to_json(&self, namespace: &str, name: &str) -> ::serde_json::Value {
        let mut document = ::serde_json::Map::new();
        document.insert("kind".to_string(), ::serde_json::json!("ConfigMap"));
        document.insert("apiVersion".to_string(), ::serde_json::json!(CORE_API_VERSION));
        document.insert(
            "metadata".to_string(),
            self.metadata.render(
                name,
                Some(namespace),
                &self.uid,
                self.created_at,
                self.resource_version,
            ),
        );
        if !self.data.is_empty() {
            document.insert("data".to_string(), ::serde_json::json!(self.data));
        }
        if !self.binary_data.is_empty() {
            document.insert(
                "binaryData".to_string(),
                ::serde_json::json!(self.binary_data),
            );
        }
        ::serde_json::Value::Object(document)
    }
}

impl ClusterObjects {
    /// `POST /api/v1/namespaces/{namespace}/configmaps`: store the ConfigMap the
    /// manifest names and answer it as the object that was stored.
    pub fn create_config_map(
        &mut self,
        namespace: &str,
        manifest: &::serde_json::Value,
        now: SystemTime,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let name = manifest_name(manifest, "ConfigMap")?;
        let data = config_map_values(&name, manifest, "data")?;
        let binary_data = config_map_values(&name, manifest, "binaryData")?;
        let stored_bytes = stored_bytes(&data) + binary_stored_bytes(&name, &binary_data)?;
        if stored_bytes > MAX_CONFIG_MAP_BYTES {
            return Err(ApiRefusal::invalid(format!(
                "ConfigMap \"{name}\" is invalid: []: Too long: must have at most \
                 {MAX_CONFIG_MAP_BYTES} bytes"
            )));
        }
        let metadata = ObjectMetadata::from_manifest("ConfigMap", &name, manifest)?;
        let resource_version = self.versions.next();
        // The manifest is read and validated first — a refused manifest stores
        // nothing wherever it was addressed — and only then is the namespace
        // resolved: an object cannot be created in a namespace that does not
        // exist.
        let held = self.namespace_for_create(namespace, CONFIG_MAPS_RESOURCE, &name)?;
        if held.config_maps.contains_key(&name) {
            return Err(ApiRefusal::already_exists(CONFIG_MAPS_RESOURCE, &name));
        }
        if held.config_maps.len() >= MAX_CONFIG_MAPS_PER_NAMESPACE {
            return Err(ApiRefusal::too_many(
                CONFIG_MAPS_RESOURCE,
                MAX_CONFIG_MAPS_PER_NAMESPACE,
            ));
        }
        held.config_maps.insert(
            name.clone(),
            ConfigMapObject {
                uid: new_object_uid(),
                created_at: now,
                resource_version,
                metadata,
                data,
                binary_data,
            },
        );
        self.config_map_json(namespace, &name)
    }

    /// `PUT /api/v1/namespaces/{namespace}/configmaps/{name}`: replace what the
    /// ConfigMap holds, keeping the identity the create minted for it — which is
    /// what an update of an object does, and how the mapping a cluster
    /// authenticates through is maintained
    /// ([`super::kubernetes_aws_auth`]).
    ///
    /// The manifest is read and validated exactly as a create's is, so an update
    /// cannot store what a create would have refused; a manifest naming a
    /// different object than the path addresses is refused the way the API server
    /// refuses one, before anything is written.
    ///
    /// A manifest carrying a `metadata.resourceVersion` asks for the write to be
    /// made only while the object still stands at that revision
    /// ([`super::kubernetes_resource_versions`]), so one carrying a revision the
    /// object has moved on from is refused and the stored object is left as it
    /// was.
    pub fn replace_config_map(
        &mut self,
        namespace: &str,
        name: &str,
        manifest: &::serde_json::Value,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let named = manifest_name(manifest, "ConfigMap")?;
        if named != name {
            return Err(ApiRefusal::bad_request(format!(
                "the name of the object ({named}) does not match the name on the URL ({name})"
            )));
        }
        let data = config_map_values(name, manifest, "data")?;
        let binary_data = config_map_values(name, manifest, "binaryData")?;
        let stored_bytes = stored_bytes(&data) + binary_stored_bytes(name, &binary_data)?;
        if stored_bytes > MAX_CONFIG_MAP_BYTES {
            return Err(ApiRefusal::invalid(format!(
                "ConfigMap \"{name}\" is invalid: []: Too long: must have at most \
                 {MAX_CONFIG_MAP_BYTES} bytes"
            )));
        }
        let metadata = ObjectMetadata::from_manifest("ConfigMap", name, manifest)?;
        let held = self
            .namespaces
            .get_mut(namespace)
            .ok_or_else(|| ApiRefusal::not_found("namespaces", namespace))?;
        let config_map = held
            .config_maps
            .get_mut(name)
            .ok_or_else(|| ApiRefusal::not_found(CONFIG_MAPS_RESOURCE, name))?;
        admits_write(
            CONFIG_MAPS_RESOURCE,
            // The core group, which a refusal names no group for.
            None,
            name,
            config_map.resource_version,
            manifest,
        )?;
        let resource_version = self.versions.next();
        // The object's own identity is the create's: a replace changes what it
        // carries, not which object it is, so the uid and the creation instant
        // stand. The revision does NOT: the object has been written, so it stands
        // at the revision of this write.
        config_map.resource_version = resource_version;
        config_map.metadata = metadata;
        config_map.data = data;
        config_map.binary_data = binary_data;
        self.config_map_json(namespace, name)
    }

    /// `GET /api/v1/namespaces/{namespace}/configmaps/{name}`.
    pub fn config_map_json(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let config_map = self
            .namespace(namespace)?
            .config_maps
            .get(name)
            .ok_or_else(|| ApiRefusal::not_found(CONFIG_MAPS_RESOURCE, name))?;
        Ok(config_map.to_json(namespace, name))
    }

    /// `GET /api/v1/namespaces/{namespace}/configmaps`: the namespace's
    /// ConfigMaps, as the `ConfigMapList` a client reads — one chunk of it
    /// ([`super::kubernetes_lists`]).
    pub fn config_map_list_json(
        &self,
        namespace: &str,
        request: &ListRequest,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let names: Vec<String> = self
            .namespace(namespace)?
            .config_maps
            .keys()
            .cloned()
            .collect();
        Ok(listed("ConfigMapList", &names, request, |name| {
            self.config_map_json(namespace, name).ok()
        }))
    }

    /// `DELETE /api/v1/namespaces/{namespace}/configmaps/{name}`: give the
    /// ConfigMap up, answered with the `Status` a delete is answered with.
    ///
    /// What read the ConfigMap keeps what it read: a workload is configured by the
    /// values it was given, and removing the object does not reach back into it.
    pub fn delete_config_map(
        &mut self,
        namespace: &str,
        name: &str,
    ) -> Result<::serde_json::Value, ApiRefusal> {
        let held = self
            .namespaces
            .get_mut(namespace)
            .ok_or_else(|| ApiRefusal::not_found("namespaces", namespace))?;
        held.config_maps
            .remove(name)
            .ok_or_else(|| ApiRefusal::not_found(CONFIG_MAPS_RESOURCE, name))?;
        Ok(deleted_status_json(CONFIG_MAPS_RESOURCE, name, None))
    }
}

/// The `member` map of a ConfigMap manifest, as the object will hold it.
///
/// Both members are string-to-string maps, so this is the shared reader of such
/// a member ([`string_value_map`]): an entry whose value is not a string is a
/// value no ConfigMap can hold and the manifest is refused rather than stored
/// with that key dropped, in the same wording a label of the same shape is
/// refused in.
fn config_map_values(
    name: &str,
    manifest: &::serde_json::Value,
    member: &str,
) -> Result<BTreeMap<String, String>, ApiRefusal> {
    string_value_map("ConfigMap", name, member, manifest.get(member))
}

/// How many bytes one map of a ConfigMap occupies: its keys and its values, which
/// is what [`MAX_CONFIG_MAP_BYTES`] bounds.
fn stored_bytes(values: &BTreeMap<String, String>) -> usize {
    values
        .iter()
        .map(|(key, value)| key.len() + value.len())
        .sum()
}

/// How many bytes one ConfigMap's `binaryData` occupies: its keys and the bytes
/// its values DECODE to, since the member carries "the binary data ... must be a
/// base64-encoded string" and what is stored is the data rather than its
/// encoding.
///
/// A value that is not base64 at all encodes no data, so the manifest is refused
/// rather than stored. The decoder is the `aws-smithy-types` one this crate
/// already encodes with; nothing here reimplements base64.
fn binary_stored_bytes(
    name: &str,
    values: &BTreeMap<String, String>,
) -> Result<usize, ApiRefusal> {
    let mut total = 0usize;
    for (key, value) in values {
        let decoded = ::aws_smithy_types::base64::decode(value).map_err(|_| {
            invalid_member(
                "ConfigMap",
                name,
                &format!("binaryData[{key}]"),
                "must be base64-encoded data",
            )
        })?;
        total = total.saturating_add(key.len()).saturating_add(decoded.len());
    }
    Ok(total)
}
