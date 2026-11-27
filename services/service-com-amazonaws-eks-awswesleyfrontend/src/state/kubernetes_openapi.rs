// SPDX-License-Identifier: CC0-1.0

//! The OpenAPI documents a cluster's Kubernetes API server PUBLISHES, which a
//! client reads before it sends a manifest.
//!
//! A Kubernetes client does not only send objects: it validates what it is about
//! to send against the schema the API server publishes for the kind. `kubectl
//! apply` reads the server's OpenAPI documents first and refuses to send a
//! manifest it cannot validate at all ("error validating data: failed to download
//! openapi"), so an API server that publishes none cannot be driven by a stock
//! client even for the objects it does serve.
//!
//! Two surfaces are published, because a client reads whichever its version
//! knows:
//!
//! * `GET /openapi/v3` is the INDEX of group versions, each with the relative URL
//!   its own document is served at, and `GET /openapi/v3/<group version>` is that
//!   document — an OpenAPI 3.0 document whose `components.schemas` carry one
//!   schema per kind, each tagged with the group, version and kind it is for
//!   (`x-kubernetes-group-version-kind`), which is how a client resolves the kind
//!   its manifest names;
//! * `GET /openapi/v2` is the single Swagger 2.0 document an older client reads,
//!   carrying the SAME schemas under `definitions`.
//!
//! Each document is published in every encoding this server writes it in, and the
//! request's `Accept` chooses: JSON, and — for the Swagger 2.0 document — the
//! Protocol Buffers encoding the Go client a stock command line is built on asks for
//! it in ([`super::kubernetes_openapi_protobuf`]). An `Accept` that admits none of
//! the encodings a document is published in is refused as unacceptable rather than
//! answered with a body the client cannot decode.
//!
//! # What a published schema declares
//!
//! A client validates the WHOLE manifest, not only the members this server reads:
//! it walks the manifest against the schema of its kind and reports a member no
//! schema declares, and then sends nothing. So the schema of a kind declares the
//! members that KIND HAS — read off the Kubernetes type, not off this server's own
//! reading of it — because a manifest that is valid for the kind is one this server
//! accepts and keeps as it was sent.
//!
//! What the schemas CONSTRAIN is what this API server does with an object: the
//! members it requires of a kind are `required` (a Pod's container list, and the
//! name and image of each container — [`super::kubernetes_pod_spec`] refuses a
//! manifest without them) and the ones it reads carry their type, while a member it
//! only keeps is declared without being narrowed. Every object it stores as it was
//! submitted carries `x-kubernetes-preserve-unknown-fields` as well, because this
//! server keeps the members it was sent rather than dropping the ones it does not
//! read. So a manifest this server would accept validates, and one it would refuse
//! does not — the document and the admission are one rule.
//!
//! That makes the published document THIS SERVER's admission surface rather than a
//! copy of upstream Kubernetes': where this server requires a member upstream
//! leaves optional (a workload's `spec` and its pod template) or bounds a value
//! upstream leaves to the cluster's capacity (a Deployment's replica count, bounded
//! because every replica is an object this store holds), the document says so, so
//! that a client validating against it is told what this server will do with the
//! manifest rather than what another server would.

use super::kubernetes_objects::CORE_API_VERSION;
use super::kubernetes_openapi_protobuf::PROTOBUF_MEDIA_TYPE;
use super::kubernetes_rbac::{RBAC_API_GROUP, RBAC_GROUP_VERSION};
use super::kubernetes_self_subject_review::{
    AUTHENTICATION_API_GROUP, AUTHENTICATION_GROUP_VERSION,
};
use super::kubernetes_subject_access_review::{
    AUTHORIZATION_API_GROUP, AUTHORIZATION_GROUP_VERSION,
};
use super::kubernetes_workloads::{
    WorkloadResource, APPS_API_GROUP, APPS_GROUP_VERSION, DEFAULT_REPLICAS,
    MAX_REPLICAS_PER_WORKLOAD,
};
use super::*;

/// The path the OpenAPI v3 INDEX is served at, as the segments of a request path.
pub const OPENAPI_V3_SEGMENT: &str = "openapi";
pub const OPENAPI_V3_VERSION: &str = "v3";

/// The path the single Swagger 2.0 document is served at.
pub const OPENAPI_V2_VERSION: &str = "v2";

/// The version of the OpenAPI specification a v3 document declares, and of the
/// Swagger specification the v2 document declares.
const OPENAPI_VERSION: &str = "3.0.0";
const SWAGGER_VERSION: &str = "2.0";

/// The `info` every document carries: the Kubernetes API, at the version of this
/// cluster's control plane.
fn info(kubernetes_version: &str) -> ::serde_json::Value {
    ::serde_json::json!({ "title": "Kubernetes", "version": format!("v{kubernetes_version}") })
}

/// The group versions this API server publishes a document for, as the index
/// names each: the path under `/openapi/v3`, the API group (empty for the core
/// one) and the version.
///
/// They are exactly the group versions the routing serves
/// ([`super::kubernetes_api`]) — the core API and every group the discovery
/// document beside it names — so a client that reads the index finds a document
/// for every group version it can address and none for one it cannot. A client
/// that cannot find the document of the group version its manifest names falls
/// back to the surface an older server published, and reports that the server
/// answers nothing it can read rather than sending the object.
fn published_group_versions() -> Vec<(String, &'static str, &'static str)> {
    vec![
        (
            format!("api/{CORE_API_VERSION}"),
            "",
            CORE_API_VERSION,
        ),
        (
            format!("apis/{APPS_API_GROUP}/{APPS_GROUP_VERSION}"),
            APPS_API_GROUP,
            APPS_GROUP_VERSION,
        ),
        (
            format!("apis/{RBAC_API_GROUP}/{RBAC_GROUP_VERSION}"),
            RBAC_API_GROUP,
            RBAC_GROUP_VERSION,
        ),
        (
            format!("apis/{AUTHENTICATION_API_GROUP}/{AUTHENTICATION_GROUP_VERSION}"),
            AUTHENTICATION_API_GROUP,
            AUTHENTICATION_GROUP_VERSION,
        ),
        (
            format!("apis/{AUTHORIZATION_API_GROUP}/{AUTHORIZATION_GROUP_VERSION}"),
            AUTHORIZATION_API_GROUP,
            AUTHORIZATION_GROUP_VERSION,
        ),
    ]
}

/// `GET /openapi/v3`: the index of the documents this server publishes, each with
/// the relative URL it is served at.
///
/// The `hash` the URL carries is the digest of the document it names, which is
/// what a client caches by: a document that has not changed carries the same hash,
/// so nothing re-reads it, and the value is derived from the document itself
/// rather than minted per call.
#[must_use]
pub fn v3_index(kubernetes_version: &str) -> ::serde_json::Value {
    let mut paths = ::serde_json::Map::new();
    for (path, group, version) in published_group_versions() {
        let document = group_version_document(kubernetes_version, group, version);
        let hash = document_hash(&document);
        paths.insert(
            path.clone(),
            ::serde_json::json!({
                "serverRelativeURL": format!("/{OPENAPI_V3_SEGMENT}/{OPENAPI_V3_VERSION}/{path}?hash={hash}"),
            }),
        );
    }
    ::serde_json::json!({ "paths": ::serde_json::Value::Object(paths) })
}

/// `GET /openapi/v3/<group version>`: the OpenAPI 3.0 document of ONE group
/// version, and `None` for a group version this server publishes none for.
///
/// `path` is the path under `/openapi/v3`, as the index names it.
#[must_use]
pub fn v3_document(kubernetes_version: &str, path: &str) -> Option<::serde_json::Value> {
    let (_, group, version) = published_group_versions()
        .into_iter()
        .find(|(published, _, _)| published == path)?;
    Some(group_version_document(kubernetes_version, group, version))
}

/// `GET /openapi/v2`: the single Swagger 2.0 document, carrying the schemas of
/// every group version this server publishes under `definitions`.
#[must_use]
pub fn v2_document(kubernetes_version: &str) -> ::serde_json::Value {
    let mut definitions = ::serde_json::Map::new();
    for (_, group, version) in published_group_versions() {
        for (name, schema) in schemas_of(group, version) {
            definitions.insert(name, swagger_references(schema));
        }
    }
    ::serde_json::json!({
        "swagger": SWAGGER_VERSION,
        "info": info(kubernetes_version),
        // This server serves its resources under the paths the routing answers;
        // the document a client validates a manifest against is the SCHEMAS, and
        // the paths of a Swagger document are not what it reads for that.
        "paths": ::serde_json::Map::new(),
        "definitions": ::serde_json::Value::Object(definitions),
    })
}

/// The OpenAPI 3.0 document of one group version.
fn group_version_document(
    kubernetes_version: &str,
    group: &str,
    version: &str,
) -> ::serde_json::Value {
    let schemas: ::serde_json::Map<String, ::serde_json::Value> =
        schemas_of(group, version).into_iter().collect();
    ::serde_json::json!({
        "openapi": OPENAPI_VERSION,
        "info": info(kubernetes_version),
        "paths": ::serde_json::Map::new(),
        "components": { "schemas": ::serde_json::Value::Object(schemas) },
    })
}

/// The digest of one published document, which is what a client caches it by.
fn document_hash(document: &::serde_json::Value) -> String {
    hex::encode(Sha256::digest(document.to_string().as_bytes()))
}

/// Every `#/components/$ref` of a v3 schema, rewritten to the `#/definitions/`
/// form a Swagger 2.0 document references its definitions by.
fn swagger_references(schema: ::serde_json::Value) -> ::serde_json::Value {
    match schema {
        ::serde_json::Value::String(value) => {
            ::serde_json::Value::String(value.replace(V3_SCHEMA_REF, SWAGGER_SCHEMA_REF))
        }
        ::serde_json::Value::Array(entries) => ::serde_json::Value::Array(
            entries.into_iter().map(swagger_references).collect(),
        ),
        ::serde_json::Value::Object(members) => ::serde_json::Value::Object(
            members
                .into_iter()
                .map(|(name, value)| (name, swagger_references(value)))
                .collect(),
        ),
        other => other,
    }
}

/// How a v3 document and a Swagger 2.0 document each reference a schema.
const V3_SCHEMA_REF: &str = "#/components/schemas/";
const SWAGGER_SCHEMA_REF: &str = "#/definitions/";

/// The schema name of one Kubernetes kind, as both documents key it: the Go
/// package path of the type, which is how a client looks a kind's schema up.
fn schema_name(group: &str, version: &str, kind: &str) -> String {
    match group {
        "" => format!("io.k8s.api.core.{version}.{kind}"),
        RBAC_API_GROUP => format!("io.k8s.api.rbac.{version}.{kind}"),
        AUTHENTICATION_API_GROUP => format!("io.k8s.api.authentication.{version}.{kind}"),
        AUTHORIZATION_API_GROUP => format!("io.k8s.api.authorization.{version}.{kind}"),
        other => format!("io.k8s.api.{other}.{version}.{kind}"),
    }
}

/// The schema name of one `apimachinery` type, which every group version's
/// objects reference.
fn meta_schema_name(kind: &str) -> String {
    format!("io.k8s.apimachinery.pkg.apis.meta.v1.{kind}")
}

/// A reference to the schema `name`.
fn reference(name: &str) -> ::serde_json::Value {
    ::serde_json::json!({ "$ref": format!("{V3_SCHEMA_REF}{name}") })
}

/// An object schema whose members this server KEEPS as it was sent them: the
/// properties it reads are declared, the ones it does not are preserved.
fn object(
    properties: ::serde_json::Value,
    required: &[&str],
) -> ::serde_json::Value {
    let mut schema = ::serde_json::json!({
        "type": "object",
        "properties": properties,
        // This server stores an object as it was submitted — nothing it does not
        // read is dropped — so the schema it publishes preserves those members
        // rather than declaring the object closed.
        "x-kubernetes-preserve-unknown-fields": true,
    });
    if !required.is_empty() {
        schema["required"] = ::serde_json::json!(required);
    }
    schema
}

/// The object schema of one KIND: an object as above, carrying the group, version
/// and kind a client resolves its manifest against, and the three members every
/// Kubernetes object has.
fn kind_schema(
    group: &str,
    version: &str,
    kind: &str,
    mut properties: ::serde_json::Map<String, ::serde_json::Value>,
    required: &[&str],
) -> ::serde_json::Value {
    properties.insert("apiVersion".to_string(), ::serde_json::json!({ "type": "string" }));
    properties.insert("kind".to_string(), ::serde_json::json!({ "type": "string" }));
    properties.insert(
        "metadata".to_string(),
        reference(&meta_schema_name("ObjectMeta")),
    );
    let mut schema = object(::serde_json::Value::Object(properties), required);
    schema["x-kubernetes-group-version-kind"] = ::serde_json::json!([{
        "group": group,
        "version": version,
        "kind": kind,
    }]);
    schema
}

/// One schema entry, as the documents key it.
fn entry(
    group: &str,
    version: &str,
    kind: &str,
    properties: &[(&str, ::serde_json::Value)],
    required: &[&str],
) -> (String, ::serde_json::Value) {
    let members: ::serde_json::Map<String, ::serde_json::Value> = properties
        .iter()
        .map(|(name, schema)| ((*name).to_string(), schema.clone()))
        .collect();
    (
        schema_name(group, version, kind),
        kind_schema(group, version, kind, members, required),
    )
}

/// A schema of an arbitrary object this server keeps as it was sent: a `status`
/// it computes, a `data` map, a nested configuration.
fn open_object() -> ::serde_json::Value {
    object(::serde_json::json!({}), &[])
}

/// A map of strings, as a label set, an annotation set and a ConfigMap's data
/// are.
fn string_map() -> ::serde_json::Value {
    ::serde_json::json!({
        "type": "object",
        "additionalProperties": { "type": "string" },
    })
}

/// A list of the schema `name`.
fn list_of(name: &str) -> ::serde_json::Value {
    ::serde_json::json!({ "type": "array", "items": reference(name) })
}

/// The JSON encoding every published document is written in, which is also the one
/// this server writes when the client states no preference.
pub const DOCUMENT_MEDIA_TYPE: &str = "application/json";

/// The encodings the Swagger 2.0 document is published in, in the order this server
/// prefers them — JSON first, because that is what a client that states no
/// preference reads.
///
/// The documents are the one surface of this API a client NEGOTIATES an encoding
/// for, and the Go client a stock Kubernetes command line is built on asks for this
/// document in the Protocol Buffers encoding ALONE, so a server that publishes it
/// only in JSON cannot be driven by that client at all
/// ([`super::kubernetes_openapi_protobuf`]).
pub const V2_MEDIA_TYPES: &[&str] = &[DOCUMENT_MEDIA_TYPE, PROTOBUF_MEDIA_TYPE];

/// The encodings the v3 index and the v3 group-version documents are published in.
pub const V3_MEDIA_TYPES: &[&str] = &[DOCUMENT_MEDIA_TYPE];

/// The encodings the document `path` addresses is published in, and `None` for a
/// path that addresses no published document.
///
/// The ONE reading of a request path as a published document, so what the routing
/// negotiates and what the answer is written in are decided alike.
#[must_use]
pub fn published_at(path: &str) -> Option<&'static [&'static str]> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    if segments.next()? != OPENAPI_V3_SEGMENT {
        return None;
    }
    match segments.next()? {
        OPENAPI_V2_VERSION => segments.next().is_none().then_some(V2_MEDIA_TYPES),
        OPENAPI_V3_VERSION => Some(V3_MEDIA_TYPES),
        _ => None,
    }
}

/// The encoding `accept` chooses out of the ones a document is `published` in, and
/// `None` when it admits none of them — the request this server answers as
/// unacceptable rather than with a body the client cannot decode.
///
/// A client states what it can read as a list of offers, each with the quality it
/// takes that one at, so the choice is the published encoding offered at the highest
/// quality, and the one the client offered first among equals. An offer of `*/*` or
/// of `application/*` leaves the choice to this server, which is the first encoding
/// the document is published in; a request that states no preference reads that same
/// encoding.
///
/// A media type offered with `q=0` is one the client states it does NOT accept ("a
/// quality value of 0 means 'not acceptable'", RFC 9110), so it chooses nothing.
///
/// The offers are read against the FIXED list of encodings a document is published
/// in rather than parsed into media types of their own: the name this server's
/// Protocol Buffers encoding is published under is not a well-formed media type —
/// its subtype carries a character a media-type parser does not admit
/// ([`super::kubernetes_openapi_protobuf::PROTOBUF_MEDIA_TYPE`]) — so a conforming
/// parser cannot hold the one name a stock client asks this document for.
#[must_use]
pub fn negotiated(accept: Option<&str>, published: &[&'static str]) -> Option<&'static str> {
    let preferred = published.first().copied();
    let Some(accept) = accept.map(str::trim).filter(|accept| !accept.is_empty()) else {
        return preferred;
    };
    let mut chosen: Option<(f32, &'static str)> = None;
    for offered in accept.split(',') {
        let mut parameters = offered.split(';');
        let media_type = parameters.next().unwrap_or_default().trim();
        // The quality the client takes this offer at, which is the last `q` it stated
        // on it; an offer states none at full quality.
        let quality = parameters
            .filter_map(|parameter| {
                let (name, value) = parameter.split_once('=')?;
                name.trim()
                    .eq_ignore_ascii_case("q")
                    .then(|| value.trim().parse::<f32>().ok())
                    .flatten()
            })
            .next_back()
            .unwrap_or(1.0);
        if quality <= 0.0 {
            continue;
        }
        let offer = if media_type == "*/*" || media_type.eq_ignore_ascii_case("application/*") {
            preferred
        } else {
            published
                .iter()
                .find(|published| published.eq_ignore_ascii_case(media_type))
                .copied()
        };
        let Some(offer) = offer else {
            continue;
        };
        // Strictly higher, so the order the client wrote its offers in decides among
        // offers it takes at the same quality.
        if chosen.is_none_or(|(taken, _)| quality > taken) {
            chosen = Some((quality, offer));
        }
    }
    chosen.map(|(_, offer)| offer)
}

/// The sentence the API server refuses an unacceptable `Accept` with, naming the
/// media types the document it addresses is published in.
#[must_use]
pub fn unacceptable_message(published: &[&str]) -> String {
    format!(
        "only the following media types are accepted: {}",
        published.join(", ")
    )
}

/// The published `document` as the bytes of the encoding the request's `accept`
/// chose, and the content type to write them with — `None` when that encoding is the
/// JSON every other answer of this API server is written in.
#[must_use]
pub fn negotiated_body(
    path: &str,
    accept: Option<&str>,
    status: u16,
    document: &::serde_json::Value,
) -> Option<(&'static str, Vec<u8>)> {
    // Only a published document is negotiated: a refusal is the `Status` object
    // every other refusal of this API server is.
    if status != 200 {
        return None;
    }
    let media_type = negotiated(accept, published_at(path)?)?;
    (media_type == PROTOBUF_MEDIA_TYPE).then(|| {
        (
            kubernetes_openapi_protobuf::PROTOBUF_CONTENT_TYPE,
            kubernetes_openapi_protobuf::encoded(document),
        )
    })
}

/// A member a published schema DECLARES without narrowing what may be in it: any
/// value validates.
///
/// Every kind this server serves has members beside the ones this server itself
/// reads, and a client validates the whole manifest against the published schema
/// before it sends it — so a member the kind has is declared even where nothing here
/// constrains it, because a manifest carrying it is one this server accepts and
/// keeps as it was sent.
fn any_value() -> ::serde_json::Value {
    ::serde_json::json!({})
}

/// A member whose value is text, a whole number, a flag, or a list of texts.
fn text_value() -> ::serde_json::Value {
    ::serde_json::json!({ "type": "string" })
}

fn number_value() -> ::serde_json::Value {
    ::serde_json::json!({ "type": "integer" })
}

fn flag_value() -> ::serde_json::Value {
    ::serde_json::json!({ "type": "boolean" })
}

fn text_list_value() -> ::serde_json::Value {
    ::serde_json::json!({ "type": "array", "items": { "type": "string" } })
}

/// The members of the object metadata every Kubernetes object carries.
///
/// SOURCE: the members the `ObjectMeta` type of the Kubernetes API machinery
/// declares. The ones this server reads and reports are typed; the rest are declared
/// as members of the kind without being narrowed ([`any_value`]).
fn object_meta_members() -> ::serde_json::Value {
    ::serde_json::json!({
        "name": text_value(),
        "generateName": text_value(),
        "namespace": text_value(),
        "selfLink": text_value(),
        "uid": text_value(),
        "resourceVersion": text_value(),
        "generation": number_value(),
        "creationTimestamp": text_value(),
        "deletionTimestamp": text_value(),
        "deletionGracePeriodSeconds": number_value(),
        "labels": string_map(),
        "annotations": string_map(),
        "ownerReferences": any_value(),
        "finalizers": text_list_value(),
        "managedFields": any_value(),
    })
}

/// The members of one container of a pod.
///
/// SOURCE: the members the core `Container` type declares. `name` and `image` are
/// the two this server reads and requires ([`super::kubernetes_pod_spec`]); the rest
/// are the container's own members, declared because a manifest that carries them is
/// one this server accepts.
fn container_members() -> ::serde_json::Value {
    ::serde_json::json!({
        "name": text_value(),
        "image": text_value(),
        "command": text_list_value(),
        "args": text_list_value(),
        "workingDir": text_value(),
        "ports": any_value(),
        "envFrom": any_value(),
        "env": any_value(),
        "resources": any_value(),
        "resizePolicy": any_value(),
        "restartPolicy": text_value(),
        "volumeMounts": any_value(),
        "volumeDevices": any_value(),
        "livenessProbe": any_value(),
        "readinessProbe": any_value(),
        "startupProbe": any_value(),
        "lifecycle": any_value(),
        "terminationMessagePath": text_value(),
        "terminationMessagePolicy": text_value(),
        "imagePullPolicy": text_value(),
        "securityContext": any_value(),
        "stdin": flag_value(),
        "stdinOnce": flag_value(),
        "tty": flag_value(),
    })
}

/// The members of a pod's specification, with the container list referencing the
/// container schema `container`.
///
/// SOURCE: the members the core `PodSpec` type declares. `containers`,
/// `serviceAccountName`, `serviceAccount` and `nodeName` are the ones this server
/// reads; the rest are the specification's own members.
fn pod_spec_members(container: &str) -> ::serde_json::Value {
    ::serde_json::json!({
        "containers": list_of(container),
        "initContainers": list_of(container),
        "ephemeralContainers": any_value(),
        "volumes": any_value(),
        "restartPolicy": text_value(),
        "terminationGracePeriodSeconds": number_value(),
        "activeDeadlineSeconds": number_value(),
        "dnsPolicy": text_value(),
        "dnsConfig": any_value(),
        "nodeSelector": string_map(),
        "serviceAccountName": text_value(),
        "serviceAccount": text_value(),
        "automountServiceAccountToken": flag_value(),
        "nodeName": text_value(),
        "hostNetwork": flag_value(),
        "hostPID": flag_value(),
        "hostIPC": flag_value(),
        "shareProcessNamespace": flag_value(),
        "securityContext": any_value(),
        "imagePullSecrets": any_value(),
        "hostname": text_value(),
        "subdomain": text_value(),
        "affinity": any_value(),
        "schedulerName": text_value(),
        "tolerations": any_value(),
        "hostAliases": any_value(),
        "priorityClassName": text_value(),
        "priority": number_value(),
        "preemptionPolicy": text_value(),
        "readinessGates": any_value(),
        "runtimeClassName": text_value(),
        "enableServiceLinks": flag_value(),
        "overhead": any_value(),
        "topologySpreadConstraints": any_value(),
        "setHostnameAsFQDN": flag_value(),
        "os": any_value(),
        "hostUsers": flag_value(),
        "schedulingGates": any_value(),
        "resourceClaims": any_value(),
        "resources": any_value(),
    })
}

/// The schemas of one group version: one per kind this server serves there, plus
/// the nested types they reference.
fn schemas_of(group: &str, version: &str) -> Vec<(String, ::serde_json::Value)> {
    let mut schemas = vec![(
        meta_schema_name("ObjectMeta"),
        object(object_meta_members(), &[]),
    )];
    match (group, version) {
        ("", _) => {
            let pod_spec = push_pod_spec_schemas(&mut schemas);
            schemas.push(entry(
                group,
                version,
                "Pod",
                &[
                    ("spec", reference(&pod_spec)),
                    ("status", open_object()),
                ],
                &["spec"],
            ));
            schemas.push(entry(group, version, "Namespace", &[("spec", open_object()), ("status", open_object())], &[]));
            schemas.push(entry(
                group,
                version,
                "ConfigMap",
                &[
                    ("data", string_map()),
                    ("binaryData", string_map()),
                    ("immutable", flag_value()),
                ],
                &[],
            ));
            schemas.push(entry(
                group,
                version,
                "ServiceAccount",
                &[
                    ("secrets", any_value()),
                    ("imagePullSecrets", any_value()),
                    ("automountServiceAccountToken", flag_value()),
                ],
                &[],
            ));
            // The Service a client names a workload by, and the ports it answers
            // on ([`super::kubernetes_services`]).
            let service_port = schema_name(group, version, "ServicePort");
            schemas.push((
                service_port.clone(),
                object(
                    ::serde_json::json!({
                        "name": text_value(),
                        "protocol": text_value(),
                        "port": number_value(),
                        // `targetPort` is an `IntOrString`: "if this is a string,
                        // it will be looked up as a named port in the target Pod's
                        // container ports". The Kubernetes schema declares such a
                        // member as a string carrying the `int-or-string` format,
                        // which is the form a client reads a member written either
                        // way by — and the one a Swagger 2.0 document can carry at
                        // all, since that specification has no alternative-schema
                        // keyword.
                        "targetPort": { "type": "string", "format": "int-or-string" },
                        "nodePort": number_value(),
                    }),
                    // The one member a Service port cannot be read without.
                    &["port"],
                ),
            ));
            let service_spec = schema_name(group, version, "ServiceSpec");
            schemas.push((
                service_spec.clone(),
                object(
                    ::serde_json::json!({
                        "type": text_value(),
                        "clusterIP": text_value(),
                        "clusterIPs": text_list_value(),
                        "externalName": text_value(),
                        "ports": list_of(&service_port),
                        "selector": string_map(),
                        "sessionAffinity": text_value(),
                    }),
                    &[],
                ),
            ));
            schemas.push(entry(
                group,
                version,
                "Service",
                &[
                    ("spec", reference(&service_spec)),
                    ("status", open_object()),
                ],
                &[],
            ));
            schemas.push(entry(
                group,
                version,
                "Event",
                // SOURCE: the members the core `Event` type declares.
                &[
                    ("involvedObject", open_object()),
                    ("reason", text_value()),
                    ("message", text_value()),
                    ("type", text_value()),
                    ("source", open_object()),
                    ("count", number_value()),
                    ("firstTimestamp", text_value()),
                    ("lastTimestamp", text_value()),
                    ("eventTime", text_value()),
                    ("action", text_value()),
                    ("reportingComponent", text_value()),
                    ("reportingInstance", text_value()),
                    ("related", any_value()),
                    ("series", any_value()),
                ],
                &[],
            ));
            schemas.push(entry(
                group,
                version,
                "Node",
                &[("spec", open_object()), ("status", open_object())],
                &[],
            ));
        }
        (RBAC_API_GROUP, _) => {
            let rule = schema_name(group, version, "PolicyRule");
            let string_list = ::serde_json::json!({
                "type": "array",
                "items": { "type": "string" },
            });
            schemas.push((
                rule.clone(),
                object(
                    ::serde_json::json!({
                        "apiGroups": string_list,
                        "resources": string_list,
                        "resourceNames": string_list,
                        "verbs": string_list,
                        "nonResourceURLs": string_list,
                    }),
                    // "Verbs is a list of Verbs that apply to ALL the ResourceKinds
                    // contained in this rule", the one member a rule states nothing
                    // without.
                    &["verbs"],
                ),
            ));
            let subject = schema_name(group, version, "Subject");
            schemas.push((
                subject.clone(),
                object(
                    ::serde_json::json!({
                        "kind": { "type": "string" },
                        "apiGroup": { "type": "string" },
                        "name": { "type": "string" },
                        "namespace": { "type": "string" },
                    }),
                    &["kind", "name"],
                ),
            ));
            let role_ref = schema_name(group, version, "RoleRef");
            schemas.push((
                role_ref.clone(),
                object(
                    ::serde_json::json!({
                        "apiGroup": { "type": "string" },
                        "kind": { "type": "string" },
                        "name": { "type": "string" },
                    }),
                    &["apiGroup", "kind", "name"],
                ),
            ));
            schemas.push(entry(group, version, "Role", &[("rules", list_of(&rule))], &[]));
            schemas.push(entry(
                group,
                version,
                "ClusterRole",
                // SOURCE: the members the `ClusterRole` type declares — a cluster
                // role aggregates other roles' rules, which a role does not.
                &[
                    ("rules", list_of(&rule)),
                    ("aggregationRule", any_value()),
                ],
                &[],
            ));
            for kind in ["RoleBinding", "ClusterRoleBinding"] {
                schemas.push(entry(
                    group,
                    version,
                    kind,
                    &[
                        ("subjects", list_of(&subject)),
                        ("roleRef", reference(&role_ref)),
                    ],
                    &["roleRef"],
                ));
            }
        }
        (AUTHENTICATION_API_GROUP, _) => {
            schemas.push(entry(
                group,
                version,
                "TokenRequest",
                &[("spec", open_object()), ("status", open_object())],
                &[],
            ));
            schemas.push(entry(
                group,
                version,
                "SelfSubjectReview",
                &[("spec", open_object()), ("status", open_object())],
                &[],
            ));
        }
        (AUTHORIZATION_API_GROUP, _) => {
            // A review states the request it asks about in its `spec`, and this
            // server refuses one that states none
            // ([`super::kubernetes_subject_access_review::NO_ATTRIBUTES_MESSAGE`]),
            // so the schema requires it; the attributes inside it are kept as they
            // were sent.
            schemas.push(entry(
                group,
                version,
                "SelfSubjectAccessReview",
                &[("spec", open_object()), ("status", open_object())],
                &["spec"],
            ));
        }
        (APPS_API_GROUP, _) => {
            // The pod template a workload runs is a POD's own spec, validated by
            // the same admission ([`super::kubernetes_workloads`]), so this
            // document carries the core group's container and pod-spec schemas
            // too: a client resolves a `$ref` inside the document it read.
            let pod_spec = push_pod_spec_schemas(&mut schemas);
            let template = schema_name("", CORE_API_VERSION, "PodTemplateSpec");
            schemas.push((
                template.clone(),
                object(
                    ::serde_json::json!({
                        "metadata": reference(&meta_schema_name("ObjectMeta")),
                        "spec": reference(&pod_spec),
                    }),
                    &[],
                ),
            ));
            let selector = meta_schema_name("LabelSelector");
            schemas.push((
                selector.clone(),
                object(::serde_json::json!({ "matchLabels": string_map() }), &[]),
            ));
            for resource in WorkloadResource::all() {
                let kind = resource.kind();
                let spec_name = schema_name(group, version, &format!("{kind}Spec"));
                let mut properties = ::serde_json::json!({
                    "selector": reference(&selector),
                    "template": reference(&template),
                    // SOURCE: the members the `DeploymentSpec` and `DaemonSetSpec`
                    // types declare beside the selector and the template. A client
                    // validates the WHOLE manifest against the schema of its kind
                    // and sends nothing when it carries a member no schema declares,
                    // so every member of the kind is declared — the rollout strategy
                    // among them, which is the member a rolling update states.
                    "minReadySeconds": number_value(),
                    "revisionHistoryLimit": number_value(),
                });
                // Only a workload that runs a NUMBER of replicas declares one, and
                // it declares the count this server runs for a spec that names none
                // and the most it serves — the bound a create past it is refused
                // with.
                if resource.runs_on_every_node() {
                    // SOURCE: `DaemonSetSpec`, whose rollout member is named for the
                    // set it updates.
                    properties["updateStrategy"] = any_value();
                } else {
                    properties["replicas"] = ::serde_json::json!({
                        "type": "integer",
                        "default": DEFAULT_REPLICAS,
                        "maximum": MAX_REPLICAS_PER_WORKLOAD,
                    });
                    // SOURCE: the members `DeploymentSpec` declares beyond the ones
                    // above.
                    properties["strategy"] = any_value();
                    properties["paused"] = flag_value();
                    properties["progressDeadlineSeconds"] = number_value();
                }
                schemas.push((
                    spec_name.clone(),
                    // The pod template is the one member a workload is refused
                    // without: "a workload with no pod template is a workload with
                    // no work".
                    object(properties, &["template"]),
                ));
                schemas.push(entry(
                    group,
                    version,
                    kind,
                    &[
                        ("spec", reference(&spec_name)),
                        ("status", open_object()),
                    ],
                    &["spec"],
                ));
            }
        }
        _ => {}
    }
    schemas
}

/// The schemas of a POD's own spec — the container list and the pod spec that
/// carries it — pushed onto `schemas`, with the name of the pod-spec schema
/// returned.
///
/// They are the CORE group's types, and the documents of every group version whose
/// objects carry a pod template declare them too: a client resolves a `$ref`
/// inside the one document it read, so a reference to a schema that document does
/// not carry resolves to nothing.
fn push_pod_spec_schemas(schemas: &mut Vec<(String, ::serde_json::Value)>) -> String {
    let container = schema_name("", CORE_API_VERSION, "Container");
    schemas.push((
        container.clone(),
        object(
            container_members(),
            // The two members this server requires of a container: a pod
            // whose container names neither has no workload to run
            // ([`super::kubernetes_pod_spec`]).
            &["name", "image"],
        ),
    ));
    let pod_spec = schema_name("", CORE_API_VERSION, "PodSpec");
    schemas.push((
        pod_spec.clone(),
        object(
            pod_spec_members(&container),
            // "There must be at least one container in a Pod".
            &["containers"],
        ),
    ));
    pod_spec
}
