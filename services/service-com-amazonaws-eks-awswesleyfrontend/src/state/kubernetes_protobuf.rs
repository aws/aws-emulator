// SPDX-License-Identifier: CC0-1.0

//! The KUBERNETES PROTOBUF encoding a client may write a write body in, read as
//! the object document the rest of this API server works in.
//!
//! A Kubernetes client states the encoding it wrote a body in on `Content-Type`,
//! and a client configured for `application/vnd.kubernetes.protobuf` — which the
//! Go client the kubectl command line is built on is, whenever its content type
//! says so — writes the object as that encoding rather than as JSON. The body is
//! then the four magic bytes that mark the encoding, followed by the envelope
//! message that names the object's `apiVersion` and `kind` and carries the object
//! itself as the bytes of its own generated message.
//!
//! The field numbers and the message shapes are the Kubernetes schema's, so they
//! are taken from the generated bindings for that schema (`k8s-pb`, decoded with
//! `prost`) rather than restated here. What this module adds is the projection of
//! a decoded message onto the SAME document a client writing JSON sends, so that
//! one reading of a manifest serves both encodings: every handler of this plane
//! keeps reading one document shape, whichever encoding the client wrote.
//!
//! The projection covers the members the objects of this plane carry
//! ([`super::kubernetes_objects`]): what a create reads, what it stores and what a
//! later read reports. A member of the Kubernetes schema this plane's objects do
//! not hold is not projected, because there is nothing here that would store it —
//! exactly as it is left unread in a JSON body.

use ::k8s_pb::api::authentication::v1 as authentication_v1;
use ::k8s_pb::api::authorization::v1 as authorization_v1;
use ::k8s_pb::api::core::v1 as core_v1;
use ::k8s_pb::api::rbac::v1 as rbac_v1;
use ::k8s_pb::apimachinery::pkg::api::resource::Quantity;
use ::k8s_pb::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use ::k8s_pb::apimachinery::pkg::runtime::Unknown;
use ::prost::Message as _;
use ::serde_json::{Map, Value};
use ::std::collections::BTreeMap;

use super::kubernetes_objects::{ApiRefusal, AUTHENTICATION_API_VERSION, CORE_API_VERSION};
use super::kubernetes_rbac::RBAC_API_VERSION;
use super::kubernetes_subject_access_review::AUTHORIZATION_API_VERSION;

/// The media type a client states when it wrote the body in this encoding.
pub const PROTOBUF_MEDIA_TYPE: &str = "application/vnd.kubernetes.protobuf";

/// The four bytes every body in this encoding opens with, which is what tells a
/// body written in it apart from one that only claims to be.
const MAGIC_PREFIX: [u8; 4] = [b'k', b'8', b's', 0x00];

/// The object `body` carries, as the document a handler of this plane reads.
///
/// A body that does not open with the encoding's own magic bytes, or whose
/// envelope does not decode, is a body the server could not read at all — the
/// `400` a body it cannot decode an object out of is answered with, as the JSON
/// reading answers one. A body that decodes but names a kind this API server has
/// no object for is the refusal the Kubernetes machinery answers an unregistered
/// kind with, naming the kind and the version it was sent for.
pub fn read_object(body: &[u8]) -> Result<Value, ApiRefusal> {
    let Some(message) = body.strip_prefix(&MAGIC_PREFIX[..]) else {
        return Err(unreadable(
            "provided data does not appear to be a protobuf message",
        ));
    };
    let envelope = Unknown::decode(message).map_err(|error| unreadable(&error.to_string()))?;
    let type_meta = envelope.type_meta.unwrap_or_default();
    let api_version = type_meta.api_version.unwrap_or_default();
    let kind = type_meta.kind.unwrap_or_default();
    let raw = envelope.raw.unwrap_or_default();
    let mut document = object_document(&api_version, &kind, &raw)?;
    // The envelope's own `apiVersion` and `kind`, which is where a body in this
    // encoding carries them: the document a client writing JSON sends carries
    // them inside the object, so the projection puts them there.
    document.insert("apiVersion".to_string(), Value::String(api_version));
    document.insert("kind".to_string(), Value::String(kind));
    Ok(Value::Object(document))
}

/// The document the object `raw` holds, projected by the kind the envelope named.
fn object_document(
    api_version: &str,
    kind: &str,
    raw: &[u8],
) -> Result<Map<String, Value>, ApiRefusal> {
    match (api_version, kind) {
        (CORE_API_VERSION, "Namespace") => {
            let object: core_v1::Namespace = decoded(raw)?;
            let mut document = metadata_document(object.metadata.as_ref());
            if let Some(spec) = object.spec {
                let mut projected = Map::new();
                put_texts(&mut projected, "finalizers", &spec.finalizers);
                document.insert("spec".to_string(), Value::Object(projected));
            }
            Ok(document)
        }
        (CORE_API_VERSION, "Pod") => {
            let object: core_v1::Pod = decoded(raw)?;
            let mut document = metadata_document(object.metadata.as_ref());
            if let Some(spec) = object.spec {
                document.insert("spec".to_string(), pod_spec(&spec));
            }
            Ok(document)
        }
        (CORE_API_VERSION, "ConfigMap") => {
            let object: core_v1::ConfigMap = decoded(raw)?;
            let mut document = metadata_document(object.metadata.as_ref());
            put_flag(&mut document, "immutable", object.immutable);
            put_text_map(&mut document, "data", &object.data);
            // A `binaryData` value is bytes in this encoding and the Base64 text
            // of those bytes in a JSON body, which is the form this plane holds
            // one in.
            let encoded: BTreeMap<String, String> = object
                .binary_data
                .iter()
                .map(|(key, value)| (key.clone(), ::aws_smithy_types::base64::encode(value)))
                .collect();
            put_text_map(&mut document, "binaryData", &encoded);
            Ok(document)
        }
        (CORE_API_VERSION, "Service") => {
            let object: core_v1::Service = decoded(raw)?;
            let mut document = metadata_document(object.metadata.as_ref());
            if let Some(spec) = object.spec {
                let mut projected = Map::new();
                put_text(&mut projected, "type", spec.r#type.as_deref());
                put_text(&mut projected, "clusterIP", spec.cluster_ip.as_deref());
                put_texts(&mut projected, "clusterIPs", &spec.cluster_ips);
                put_text(&mut projected, "externalName", spec.external_name.as_deref());
                put_text_map(&mut projected, "selector", &spec.selector);
                let ports: Vec<Value> = spec.ports.iter().map(service_port).collect();
                put_values(&mut projected, "ports", ports);
                document.insert("spec".to_string(), Value::Object(projected));
            }
            Ok(document)
        }
        (CORE_API_VERSION, "ServiceAccount") => {
            let object: core_v1::ServiceAccount = decoded(raw)?;
            let mut document = metadata_document(object.metadata.as_ref());
            let secrets: Vec<Value> = object
                .secrets
                .iter()
                .map(|reference| {
                    let mut projected = Map::new();
                    put_text(&mut projected, "name", reference.name.as_deref());
                    put_text(&mut projected, "namespace", reference.namespace.as_deref());
                    Value::Object(projected)
                })
                .collect();
            put_values(&mut document, "secrets", secrets);
            put_values(
                &mut document,
                "imagePullSecrets",
                local_references(&object.image_pull_secrets),
            );
            put_flag(
                &mut document,
                "automountServiceAccountToken",
                object.automount_service_account_token,
            );
            Ok(document)
        }
        (RBAC_API_VERSION, "Role") => {
            let object: rbac_v1::Role = decoded(raw)?;
            let mut document = metadata_document(object.metadata.as_ref());
            put_values(&mut document, "rules", policy_rules(&object.rules));
            Ok(document)
        }
        (RBAC_API_VERSION, "ClusterRole") => {
            let object: rbac_v1::ClusterRole = decoded(raw)?;
            let mut document = metadata_document(object.metadata.as_ref());
            put_values(&mut document, "rules", policy_rules(&object.rules));
            Ok(document)
        }
        (RBAC_API_VERSION, "RoleBinding") => {
            let object: rbac_v1::RoleBinding = decoded(raw)?;
            let mut document = metadata_document(object.metadata.as_ref());
            binding_members(&mut document, object.role_ref.as_ref(), &object.subjects);
            Ok(document)
        }
        (RBAC_API_VERSION, "ClusterRoleBinding") => {
            let object: rbac_v1::ClusterRoleBinding = decoded(raw)?;
            let mut document = metadata_document(object.metadata.as_ref());
            binding_members(&mut document, object.role_ref.as_ref(), &object.subjects);
            Ok(document)
        }
        (AUTHENTICATION_API_VERSION, "TokenRequest") => {
            let object: authentication_v1::TokenRequest = decoded(raw)?;
            let mut document = metadata_document(object.metadata.as_ref());
            if let Some(spec) = object.spec {
                let mut projected = Map::new();
                put_texts(&mut projected, "audiences", &spec.audiences);
                put_number(
                    &mut projected,
                    "expirationSeconds",
                    spec.expiration_seconds,
                );
                if let Some(bound) = spec.bound_object_ref {
                    let mut reference = Map::new();
                    put_text(&mut reference, "kind", bound.kind.as_deref());
                    put_text(&mut reference, "apiVersion", bound.api_version.as_deref());
                    put_text(&mut reference, "name", bound.name.as_deref());
                    put_text(&mut reference, "uid", bound.u_id.as_deref());
                    projected.insert("boundObjectRef".to_string(), Value::Object(reference));
                }
                document.insert("spec".to_string(), Value::Object(projected));
            }
            Ok(document)
        }
        (AUTHENTICATION_API_VERSION, "SelfSubjectReview") => {
            let object: authentication_v1::SelfSubjectReview = decoded(raw)?;
            Ok(metadata_document(object.metadata.as_ref()))
        }
        // The reviews a client asks the authorizer with: what the review ASKS about
        // is its `spec`, which is what the decision is taken over
        // ([`super::kubernetes_subject_access_review`]).
        (AUTHORIZATION_API_VERSION, "SelfSubjectAccessReview") => {
            let object: authorization_v1::SelfSubjectAccessReview = decoded(raw)?;
            let mut document = metadata_document(object.metadata.as_ref());
            if let Some(spec) = object.spec {
                let mut projected = Map::new();
                if let Some(resource) = spec.resource_attributes {
                    let mut asked = Map::new();
                    put_text(&mut asked, "namespace", resource.namespace.as_deref());
                    put_text(&mut asked, "verb", resource.verb.as_deref());
                    put_text(&mut asked, "group", resource.group.as_deref());
                    put_text(&mut asked, "version", resource.version.as_deref());
                    put_text(&mut asked, "resource", resource.resource.as_deref());
                    put_text(&mut asked, "subresource", resource.subresource.as_deref());
                    put_text(&mut asked, "name", resource.name.as_deref());
                    projected.insert("resourceAttributes".to_string(), Value::Object(asked));
                }
                if let Some(non_resource) = spec.non_resource_attributes {
                    let mut asked = Map::new();
                    put_text(&mut asked, "path", non_resource.path.as_deref());
                    put_text(&mut asked, "verb", non_resource.verb.as_deref());
                    projected.insert("nonResourceAttributes".to_string(), Value::Object(asked));
                }
                document.insert("spec".to_string(), Value::Object(projected));
            }
            Ok(document)
        }
        (AUTHORIZATION_API_VERSION, "SelfSubjectRulesReview") => {
            let object: authorization_v1::SelfSubjectRulesReview = decoded(raw)?;
            let mut document = metadata_document(object.metadata.as_ref());
            if let Some(spec) = object.spec {
                let mut projected = Map::new();
                put_text(&mut projected, "namespace", spec.namespace.as_deref());
                document.insert("spec".to_string(), Value::Object(projected));
            }
            Ok(document)
        }
        _ => Err(ApiRefusal::bad_request(format!(
            "no kind \"{kind}\" is registered for version \"{api_version}\""
        ))),
    }
}

/// One port of a Service, as the document carries it
/// ([`super::kubernetes_services`]).
///
/// `targetPort` is written either as the NUMBER of a port or as the NAME of a
/// container port, which is the reading [`super::probe::port_value`] already makes
/// for the port a health probe addresses.
fn service_port(port: &core_v1::ServicePort) -> Value {
    let mut projected = Map::new();
    put_text(&mut projected, "name", port.name.as_deref());
    put_text(&mut projected, "protocol", port.protocol.as_deref());
    put_number(&mut projected, "port", port.port);
    if let Some(target) = port.target_port.as_ref().map(super::probe::port_value) {
        projected.insert("targetPort".to_string(), target);
    }
    put_number(&mut projected, "nodePort", port.node_port);
    Value::Object(projected)
}

/// One message of the Kubernetes schema, decoded out of the envelope's bytes.
fn decoded<Message: ::prost::Message + Default>(raw: &[u8]) -> Result<Message, ApiRefusal> {
    Message::decode(raw).map_err(|error| unreadable(&error.to_string()))
}

/// The `Status` a body this server could not decode an object out of is answered
/// with, in the wording the JSON reading of a body answers one with: the `400` of
/// a request it could not read at all, naming what the decoder reported.
fn unreadable(detail: &str) -> ApiRefusal {
    ApiRefusal::bad_request(format!(
        "couldn't get version/kind; protobuf parse error: {detail}"
    ))
}

/// The document an object with `metadata` opens with: the `metadata` members a
/// client submits and reads back on an object of this plane
/// ([`super::kubernetes_metadata`]).
fn metadata_document(metadata: Option<&ObjectMeta>) -> Map<String, Value> {
    let mut document = Map::new();
    let mut projected = Map::new();
    if let Some(metadata) = metadata {
        put_text(&mut projected, "name", metadata.name.as_deref());
        put_text(
            &mut projected,
            "generateName",
            metadata.generate_name.as_deref(),
        );
        put_text(&mut projected, "namespace", metadata.namespace.as_deref());
        // The revision a write states it is to be made at, which a client writing
        // this encoding states here ([`super::kubernetes_resource_versions`]): a
        // member dropped on the way in would turn a versioned write into an
        // unconditional one.
        put_text(
            &mut projected,
            "resourceVersion",
            metadata.resource_version.as_deref(),
        );
        put_text_map(&mut projected, "labels", &metadata.labels);
        put_text_map(&mut projected, "annotations", &metadata.annotations);
        put_texts(&mut projected, "finalizers", &metadata.finalizers);
    }
    document.insert("metadata".to_string(), Value::Object(projected));
    document
}

/// A Pod's `spec`, as the document the pod is admitted and stored with
/// ([`super::kubernetes_pod_spec`]).
fn pod_spec(spec: &core_v1::PodSpec) -> Value {
    let mut projected = Map::new();
    put_values(&mut projected, "volumes", volumes(&spec.volumes));
    put_values(
        &mut projected,
        "initContainers",
        containers(&spec.init_containers),
    );
    put_values(&mut projected, "containers", containers(&spec.containers));
    put_text(&mut projected, "restartPolicy", spec.restart_policy.as_deref());
    put_number(
        &mut projected,
        "terminationGracePeriodSeconds",
        spec.termination_grace_period_seconds,
    );
    put_number(
        &mut projected,
        "activeDeadlineSeconds",
        spec.active_deadline_seconds,
    );
    put_text(&mut projected, "dnsPolicy", spec.dns_policy.as_deref());
    put_text_map(&mut projected, "nodeSelector", &spec.node_selector);
    put_text(
        &mut projected,
        "serviceAccountName",
        spec.service_account_name.as_deref(),
    );
    put_text(
        &mut projected,
        "serviceAccount",
        spec.service_account.as_deref(),
    );
    put_flag(
        &mut projected,
        "automountServiceAccountToken",
        spec.automount_service_account_token,
    );
    put_text(&mut projected, "nodeName", spec.node_name.as_deref());
    put_flag(&mut projected, "hostNetwork", spec.host_network);
    put_values(
        &mut projected,
        "imagePullSecrets",
        local_references(&spec.image_pull_secrets),
    );
    put_text(&mut projected, "hostname", spec.hostname.as_deref());
    put_text(&mut projected, "subdomain", spec.subdomain.as_deref());
    put_text(
        &mut projected,
        "schedulerName",
        spec.scheduler_name.as_deref(),
    );
    put_values(&mut projected, "tolerations", tolerations(&spec.tolerations));
    put_text(
        &mut projected,
        "priorityClassName",
        spec.priority_class_name.as_deref(),
    );
    put_text(
        &mut projected,
        "runtimeClassName",
        spec.runtime_class_name.as_deref(),
    );
    put_flag(
        &mut projected,
        "enableServiceLinks",
        spec.enable_service_links,
    );
    Value::Object(projected)
}

/// A pod's containers, as the documents the container list carries.
fn containers(declared: &[core_v1::Container]) -> Vec<Value> {
    declared
        .iter()
        .map(|container| {
            let mut projected = Map::new();
            put_text(&mut projected, "name", container.name.as_deref());
            put_text(&mut projected, "image", container.image.as_deref());
            put_texts(&mut projected, "command", &container.command);
            put_texts(&mut projected, "args", &container.args);
            put_text(
                &mut projected,
                "workingDir",
                container.working_dir.as_deref(),
            );
            put_values(&mut projected, "ports", container_ports(&container.ports));
            put_values(
                &mut projected,
                "envFrom",
                environment_sources(&container.env_from),
            );
            put_values(&mut projected, "env", environment(&container.env));
            if let Some(resources) = &container.resources {
                let mut required = Map::new();
                put_quantity_map(&mut required, "limits", &resources.limits);
                put_quantity_map(&mut required, "requests", &resources.requests);
                if !required.is_empty() {
                    projected.insert("resources".to_string(), Value::Object(required));
                }
            }
            put_values(
                &mut projected,
                "volumeMounts",
                volume_mounts(&container.volume_mounts),
            );
            put_text(
                &mut projected,
                "imagePullPolicy",
                container.image_pull_policy.as_deref(),
            );
            // The health probes the kubelet performs on this container, each a
            // tree of its own ([`super::probe`]).
            for (member, probe) in [
                ("livenessProbe", container.liveness_probe.as_ref()),
                ("readinessProbe", container.readiness_probe.as_ref()),
                ("startupProbe", container.startup_probe.as_ref()),
            ] {
                if let Some(probe) = probe {
                    projected.insert(member.to_string(), super::probe::projected(probe));
                }
            }
            Value::Object(projected)
        })
        .collect()
}

/// A container's ports.
fn container_ports(declared: &[core_v1::ContainerPort]) -> Vec<Value> {
    declared
        .iter()
        .map(|port| {
            let mut projected = Map::new();
            put_text(&mut projected, "name", port.name.as_deref());
            put_number(&mut projected, "hostPort", port.host_port);
            put_number(&mut projected, "containerPort", port.container_port);
            put_text(&mut projected, "protocol", port.protocol.as_deref());
            put_text(&mut projected, "hostIP", port.host_ip.as_deref());
            Value::Object(projected)
        })
        .collect()
}

/// A container's environment: each variable's own value, or the object it reads
/// its value from.
fn environment(declared: &[core_v1::EnvVar]) -> Vec<Value> {
    declared
        .iter()
        .map(|variable| {
            let mut projected = Map::new();
            put_text(&mut projected, "name", variable.name.as_deref());
            put_text(&mut projected, "value", variable.value.as_deref());
            if let Some(source) = &variable.value_from {
                let mut from = Map::new();
                if let Some(selector) = &source.config_map_key_ref {
                    let mut reference = Map::new();
                    put_text(
                        &mut reference,
                        "name",
                        selector
                            .local_object_reference
                            .as_ref()
                            .and_then(|local| local.name.as_deref()),
                    );
                    put_text(&mut reference, "key", selector.key.as_deref());
                    put_flag(&mut reference, "optional", selector.optional);
                    from.insert("configMapKeyRef".to_string(), Value::Object(reference));
                }
                if let Some(selector) = &source.secret_key_ref {
                    let mut reference = Map::new();
                    put_text(
                        &mut reference,
                        "name",
                        selector
                            .local_object_reference
                            .as_ref()
                            .and_then(|local| local.name.as_deref()),
                    );
                    put_text(&mut reference, "key", selector.key.as_deref());
                    put_flag(&mut reference, "optional", selector.optional);
                    from.insert("secretKeyRef".to_string(), Value::Object(reference));
                }
                if !from.is_empty() {
                    projected.insert("valueFrom".to_string(), Value::Object(from));
                }
            }
            Value::Object(projected)
        })
        .collect()
}

/// A container's `envFrom`: the whole objects its environment is read from.
fn environment_sources(declared: &[core_v1::EnvFromSource]) -> Vec<Value> {
    declared
        .iter()
        .map(|source| {
            let mut projected = Map::new();
            put_text(&mut projected, "prefix", source.prefix.as_deref());
            if let Some(from) = &source.config_map_ref {
                let mut reference = Map::new();
                put_text(
                    &mut reference,
                    "name",
                    from.local_object_reference
                        .as_ref()
                        .and_then(|local| local.name.as_deref()),
                );
                put_flag(&mut reference, "optional", from.optional);
                projected.insert("configMapRef".to_string(), Value::Object(reference));
            }
            if let Some(from) = &source.secret_ref {
                let mut reference = Map::new();
                put_text(
                    &mut reference,
                    "name",
                    from.local_object_reference
                        .as_ref()
                        .and_then(|local| local.name.as_deref()),
                );
                put_flag(&mut reference, "optional", from.optional);
                projected.insert("secretRef".to_string(), Value::Object(reference));
            }
            Value::Object(projected)
        })
        .collect()
}

/// A container's volume mounts.
fn volume_mounts(declared: &[core_v1::VolumeMount]) -> Vec<Value> {
    declared
        .iter()
        .map(|mount| {
            let mut projected = Map::new();
            put_text(&mut projected, "name", mount.name.as_deref());
            put_flag(&mut projected, "readOnly", mount.read_only);
            put_text(&mut projected, "mountPath", mount.mount_path.as_deref());
            put_text(&mut projected, "subPath", mount.sub_path.as_deref());
            Value::Object(projected)
        })
        .collect()
}

/// A pod's volumes: each volume's name and the source it is filled from — the
/// sources this plane's objects are filled from, which are the cluster's own
/// objects and the empty directory a pod gets to itself.
fn volumes(declared: &[core_v1::Volume]) -> Vec<Value> {
    declared
        .iter()
        .map(|volume| {
            let mut projected = Map::new();
            put_text(&mut projected, "name", volume.name.as_deref());
            let Some(source) = &volume.volume_source else {
                return Value::Object(projected);
            };
            if let Some(from) = &source.config_map {
                let mut reference = Map::new();
                put_text(
                    &mut reference,
                    "name",
                    from.local_object_reference
                        .as_ref()
                        .and_then(|local| local.name.as_deref()),
                );
                put_values(&mut reference, "items", key_paths(&from.items));
                put_number(&mut reference, "defaultMode", from.default_mode);
                put_flag(&mut reference, "optional", from.optional);
                projected.insert("configMap".to_string(), Value::Object(reference));
            }
            if let Some(from) = &source.secret {
                let mut reference = Map::new();
                put_text(&mut reference, "secretName", from.secret_name.as_deref());
                put_values(&mut reference, "items", key_paths(&from.items));
                put_number(&mut reference, "defaultMode", from.default_mode);
                put_flag(&mut reference, "optional", from.optional);
                projected.insert("secret".to_string(), Value::Object(reference));
            }
            if let Some(from) = &source.empty_dir {
                let mut reference = Map::new();
                put_text(&mut reference, "medium", from.medium.as_deref());
                if let Some(limit) = from.size_limit.as_ref().and_then(|limit| limit.string.as_deref())
                {
                    reference.insert("sizeLimit".to_string(), Value::String(limit.to_string()));
                }
                projected.insert("emptyDir".to_string(), Value::Object(reference));
            }
            Value::Object(projected)
        })
        .collect()
}

/// The keys of an object a volume projects, each at the path it is projected to.
fn key_paths(declared: &[core_v1::KeyToPath]) -> Vec<Value> {
    declared
        .iter()
        .map(|item| {
            let mut projected = Map::new();
            put_text(&mut projected, "key", item.key.as_deref());
            put_text(&mut projected, "path", item.path.as_deref());
            put_number(&mut projected, "mode", item.mode);
            Value::Object(projected)
        })
        .collect()
}

/// A pod's tolerations: which taints of a node it tolerates.
fn tolerations(declared: &[core_v1::Toleration]) -> Vec<Value> {
    declared
        .iter()
        .map(|toleration| {
            let mut projected = Map::new();
            put_text(&mut projected, "key", toleration.key.as_deref());
            put_text(&mut projected, "operator", toleration.operator.as_deref());
            put_text(&mut projected, "value", toleration.value.as_deref());
            put_text(&mut projected, "effect", toleration.effect.as_deref());
            put_number(
                &mut projected,
                "tolerationSeconds",
                toleration.toleration_seconds,
            );
            Value::Object(projected)
        })
        .collect()
}

/// A list of references to objects of the same namespace, which is how a pull
/// secret list names its secrets.
fn local_references(declared: &[core_v1::LocalObjectReference]) -> Vec<Value> {
    declared
        .iter()
        .map(|reference| {
            let mut projected = Map::new();
            put_text(&mut projected, "name", reference.name.as_deref());
            Value::Object(projected)
        })
        .collect()
}

/// The rules of an RBAC role ([`super::kubernetes_rbac`]).
fn policy_rules(declared: &[rbac_v1::PolicyRule]) -> Vec<Value> {
    declared
        .iter()
        .map(|rule| {
            let mut projected = Map::new();
            put_texts(&mut projected, "verbs", &rule.verbs);
            put_texts(&mut projected, "apiGroups", &rule.api_groups);
            put_texts(&mut projected, "resources", &rule.resources);
            put_texts(&mut projected, "resourceNames", &rule.resource_names);
            put_texts(&mut projected, "nonResourceURLs", &rule.non_resource_ur_ls);
            Value::Object(projected)
        })
        .collect()
}

/// The two members a role binding is read by: the role it binds and the subjects
/// it binds that role to.
fn binding_members(
    document: &mut Map<String, Value>,
    role_ref: Option<&rbac_v1::RoleRef>,
    subjects: &[rbac_v1::Subject],
) {
    if let Some(role_ref) = role_ref {
        let mut reference = Map::new();
        put_text(&mut reference, "apiGroup", role_ref.api_group.as_deref());
        put_text(&mut reference, "kind", role_ref.kind.as_deref());
        put_text(&mut reference, "name", role_ref.name.as_deref());
        document.insert("roleRef".to_string(), Value::Object(reference));
    }
    let bound: Vec<Value> = subjects
        .iter()
        .map(|subject| {
            let mut projected = Map::new();
            put_text(&mut projected, "kind", subject.kind.as_deref());
            put_text(&mut projected, "apiGroup", subject.api_group.as_deref());
            put_text(&mut projected, "name", subject.name.as_deref());
            put_text(&mut projected, "namespace", subject.namespace.as_deref());
            Value::Object(projected)
        })
        .collect();
    put_values(document, "subjects", bound);
}

/// Put the text `member` carries, and nothing for a member the object did not
/// carry: a member absent from the message is absent from the document, which is
/// what a client writing JSON leaves out.
pub(super) fn put_text(into: &mut Map<String, Value>, member: &str, value: Option<&str>) {
    if let Some(value) = value {
        into.insert(member.to_string(), Value::String(value.to_string()));
    }
}

/// [`put_text`] for a repeated member: an empty list is a member the object does
/// not carry.
pub(super) fn put_texts(into: &mut Map<String, Value>, member: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    into.insert(
        member.to_string(),
        Value::Array(
            values
                .iter()
                .map(|value| Value::String(value.clone()))
                .collect(),
        ),
    );
}

/// [`put_text`] for a member holding projected documents.
pub(super) fn put_values(into: &mut Map<String, Value>, member: &str, values: Vec<Value>) {
    if values.is_empty() {
        return;
    }
    into.insert(member.to_string(), Value::Array(values));
}

/// [`put_text`] for a string-to-string map member. The entries are put in key
/// order, so one object read twice is one document.
fn put_text_map<'entries, Entries>(into: &mut Map<String, Value>, member: &str, entries: Entries)
where
    Entries: IntoIterator<Item = (&'entries String, &'entries String)>,
{
    let ordered: BTreeMap<&str, &str> = entries
        .into_iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    if ordered.is_empty() {
        return;
    }
    into.insert(
        member.to_string(),
        Value::Object(
            ordered
                .into_iter()
                .map(|(key, value)| (key.to_string(), Value::String(value.to_string())))
                .collect(),
        ),
    );
}

/// [`put_text_map`] for a map of resource quantities, each of which is the text
/// its own amount is written as.
fn put_quantity_map<'entries, Entries>(
    into: &mut Map<String, Value>,
    member: &str,
    entries: Entries,
) where
    Entries: IntoIterator<Item = (&'entries String, &'entries Quantity)>,
{
    let ordered: BTreeMap<&str, &str> = entries
        .into_iter()
        .filter_map(|(key, quantity)| {
            quantity
                .string
                .as_deref()
                .map(|amount| (key.as_str(), amount))
        })
        .collect();
    if ordered.is_empty() {
        return;
    }
    into.insert(
        member.to_string(),
        Value::Object(
            ordered
                .into_iter()
                .map(|(key, amount)| (key.to_string(), Value::String(amount.to_string())))
                .collect(),
        ),
    );
}

/// [`put_text`] for a boolean member.
fn put_flag(into: &mut Map<String, Value>, member: &str, value: Option<bool>) {
    if let Some(value) = value {
        into.insert(member.to_string(), Value::Bool(value));
    }
}

/// [`put_text`] for a whole-number member.
pub(super) fn put_number(into: &mut Map<String, Value>, member: &str, value: Option<impl Into<i64>>) {
    if let Some(value) = value {
        into.insert(member.to_string(), Value::from(value.into()));
    }
}
