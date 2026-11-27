// SPDX-License-Identifier: CC0-1.0

//! The cluster's AUDIT log: one record per request its Kubernetes API server
//! served, delivered to the cluster's own CloudWatch Logs group.
//!
//! `LogSetup$enabled` is "If a log type is enabled, that log type exports its
//! control plane logs to CloudWatch Logs", and the `audit` type's control plane
//! component is the API server's audit log — so a cluster with that type enabled
//! exports what its API server audits, which is one record per request: who made it,
//! what it addressed, whether the authorizer admitted it and what the server
//! answered. A cluster that reports `audit` enabled and exports nothing of the
//! requests it served is a cluster whose setting had no effect, which is what an
//! operator auditing who did what reads an empty log group for.
//!
//! The record is the Kubernetes AUDIT EVENT a client reads out of such a log group
//! (`audit.k8s.io/v1`, `kind: Event`): the `user` is the identity the cluster's
//! access entry mapped the IAM principal to
//! ([`super::kubernetes_authentication`]), the `objectRef` and the `verb` are what
//! the request addressed ([`super::kubernetes_rbac::request_attributes`], the same
//! reading the authorizer made its decision on), the `responseStatus.code` is the
//! status the request was answered with, and the authorizer's decision travels in
//! the annotation Kubernetes records it under.
//!
//! The group and the stream are the ones this cluster's control plane logs already
//! go to ([`super::cluster_log_group_name`],
//! [`super::control_plane_log_stream_name`]), so one cluster has one audit stream
//! however many requests it serves, and the export goes out over the same in-process
//! callback the rest of this service's cross-service work uses. It is BEST EFFORT,
//! like every other control-plane log export: a write CloudWatch Logs refuses is
//! reported in this service's own tracing rather than failing the Kubernetes request,
//! which was served either way.

use super::kubernetes_objects::{new_object_uid, rfc3339_seconds};
use super::kubernetes_rbac::{RequestAttributes, SubjectIdentity};
use super::ComAmazonawsEksAwswesleyfrontendState as ServiceStore;
use ::std::time::SystemTime;

/// The control plane log type whose records this module delivers, as
/// [`crate::ops::types::LOG_TYPE_VALUES`] spells it.
pub const AUDIT_LOG_TYPE: &str = "audit";

/// The `apiVersion` a Kubernetes audit event carries.
const AUDIT_API_VERSION: &str = "audit.k8s.io/v1";

/// The audit level the record is written at: the request's metadata — who, what,
/// when and the answer — and neither the request nor the response body, which is the
/// level a cluster's audit log records a request at by default.
const AUDIT_LEVEL: &str = "Metadata";

/// The stage of the request the record is written at: the answer has been sent, so
/// the record carries the status the request was answered with.
const AUDIT_STAGE: &str = "ResponseComplete";

/// The annotation the authorizer's decision travels in, and the two values it takes.
const AUTHORIZATION_DECISION_ANNOTATION: &str = "authorization.k8s.io/decision";
/// See [`AUTHORIZATION_DECISION_ANNOTATION`].
const DECISION_ALLOW: &str = "allow";
/// See [`AUTHORIZATION_DECISION_ANNOTATION`].
const DECISION_FORBID: &str = "forbid";

/// The CloudWatch Logs side of ONE cluster's audit log, held for as long as that
/// cluster's API address is open.
///
/// It carries the callback into the emulator this service runs within, which is the
/// seam a cross-service call goes over, plus the two values that name the group and
/// the stream.
#[derive(Clone)]
pub struct ClusterAuditLog {
    callback: ::smithy_server_runtime::EmbeddedEmulator,
    region: String,
    cluster_name: String,
}

/// Written by hand rather than derived: the callback holds the emulator's own
/// credentials, which go into no log and no response.
impl ::std::fmt::Debug for ClusterAuditLog {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("ClusterAuditLog")
            .field("region", &self.region)
            .field("cluster_name", &self.cluster_name)
            .finish_non_exhaustive()
    }
}

impl ClusterAuditLog {
    /// The audit log of the cluster `cluster_name` in `region`, exported over
    /// `callback`.
    #[must_use]
    pub fn new(
        callback: ::smithy_server_runtime::EmbeddedEmulator,
        region: &str,
        cluster_name: &str,
    ) -> Self {
        Self {
            callback,
            region: region.to_string(),
            cluster_name: cluster_name.to_string(),
        }
    }

    /// The delivery one served request owes the cluster's audit log, or `None` when
    /// the cluster does not have the `audit` log type enabled — in which case nothing
    /// is exported, which is what makes a disabled type observably different from an
    /// enabled one.
    ///
    /// Read with the store's lock held; the delivery itself runs with none.
    #[must_use]
    pub fn delivery(
        &self,
        store: &ServiceStore,
        key: &(String, String),
        record: &AuditedRequest,
    ) -> Option<AuditDelivery> {
        let cluster = store.clusters.get(key)?;
        if !super::enabled_log_types(cluster.logging.as_ref()).contains(&AUDIT_LOG_TYPE) {
            return None;
        }
        let resource_id = super::derive_cluster_resource_id(&cluster.arn);
        Some(AuditDelivery {
            log: self.clone(),
            log_group: super::cluster_log_group_name(&self.cluster_name),
            log_stream: super::control_plane_log_stream_name(AUDIT_LOG_TYPE, &resource_id)?,
            event: record.to_event(),
            at: record.answered_at,
        })
    }
}

/// One request the API server served, as the audit log records it.
#[derive(Debug, Clone)]
pub struct AuditedRequest {
    /// The identity the cluster mapped the calling IAM principal to, and `None` for
    /// a request whose credential it mapped to nobody — which the record still
    /// carries, as the API server audits a request it answered `401`.
    pub identity: Option<SubjectIdentity>,
    /// What the request addressed.
    pub attributes: RequestAttributes,
    /// The name of the object the answer carries, for a request whose PATH named none
    /// — a create, whose object is named by the manifest it carries and by the name the
    /// server minted for a `generateName`. The audit record names the object the
    /// request really acted on, which for a create is the one that was stored.
    pub answered_object_name: Option<String>,
    /// The HTTP status it was answered with.
    pub status: u16,
    /// Whether the authorizer admitted it.
    pub admitted: bool,
    /// The `User-Agent` the client sent, when it sent one.
    pub user_agent: Option<String>,
    pub received_at: SystemTime,
    pub answered_at: SystemTime,
}

/// The username a request whose credential maps to no Kubernetes identity is
/// recorded under: the user Kubernetes names an unauthenticated request's identity.
const ANONYMOUS_USERNAME: &str = "system:anonymous";

/// The group an unauthenticated request is in.
const UNAUTHENTICATED_GROUP: &str = "system:unauthenticated";

impl AuditedRequest {
    /// This request as the Kubernetes audit event a client reads out of the log
    /// group.
    #[must_use]
    fn to_event(&self) -> ::serde_json::Value {
        // The `user` of an audit event is the identity the request was served as,
        // written as the same `UserInfo` the API server reports about it
        // ([`super::kubernetes_self_subject_review::user_info`]), so an audit record
        // and a self subject review of one request describe one subject. A request
        // whose credential mapped to nobody was served as no identity at all, and is
        // recorded under the user Kubernetes names an unauthenticated request.
        let user = match &self.identity {
            Some(identity) => super::kubernetes_self_subject_review::user_info(identity),
            None => ::serde_json::json!({
                "username": ANONYMOUS_USERNAME,
                "groups": [UNAUTHENTICATED_GROUP],
            }),
        };
        let mut event = ::serde_json::Map::new();
        event.insert("kind".to_string(), ::serde_json::json!("Event"));
        event.insert(
            "apiVersion".to_string(),
            ::serde_json::json!(AUDIT_API_VERSION),
        );
        event.insert("level".to_string(), ::serde_json::json!(AUDIT_LEVEL));
        // Minted per record, never derived from the request: two requests that look
        // alike are two audited events.
        event.insert("auditID".to_string(), ::serde_json::json!(new_object_uid()));
        event.insert("stage".to_string(), ::serde_json::json!(AUDIT_STAGE));
        event.insert(
            "requestURI".to_string(),
            ::serde_json::json!(self.attributes.path),
        );
        event.insert("verb".to_string(), ::serde_json::json!(self.attributes.verb));
        event.insert("user".to_string(), user);
        if let Some(user_agent) = &self.user_agent {
            event.insert("userAgent".to_string(), ::serde_json::json!(user_agent));
        }
        if self.attributes.is_resource_request() {
            let mut object_ref = ::serde_json::Map::new();
            object_ref.insert(
                "resource".to_string(),
                ::serde_json::json!(self.attributes.resource),
            );
            let name = match (self.attributes.name.is_empty(), &self.answered_object_name) {
                (true, Some(answered)) => answered.clone(),
                _ => self.attributes.name.clone(),
            };
            for (member, value) in [
                ("namespace", &self.attributes.namespace),
                ("name", &name),
                ("apiGroup", &self.attributes.api_group),
                ("subresource", &self.attributes.subresource),
            ] {
                if !value.is_empty() {
                    object_ref.insert(member.to_string(), ::serde_json::json!(value));
                }
            }
            event.insert(
                "objectRef".to_string(),
                ::serde_json::Value::Object(object_ref),
            );
        }
        event.insert(
            "responseStatus".to_string(),
            ::serde_json::json!({ "metadata": {}, "code": self.status }),
        );
        event.insert(
            "requestReceivedTimestamp".to_string(),
            ::serde_json::json!(rfc3339_seconds(self.received_at)),
        );
        event.insert(
            "stageTimestamp".to_string(),
            ::serde_json::json!(rfc3339_seconds(self.answered_at)),
        );
        // The authorizer's decision, and NOTHING for a request that never reached it:
        // a credential the cluster maps to no identity is refused before any decision
        // is taken, which is what an API server records for one.
        if self.identity.is_some() {
            event.insert(
                "annotations".to_string(),
                ::serde_json::json!({
                    AUTHORIZATION_DECISION_ANNOTATION: if self.admitted {
                        DECISION_ALLOW
                    } else {
                        DECISION_FORBID
                    },
                }),
            );
        }
        ::serde_json::Value::Object(event)
    }
}

/// One audit record, with everything its delivery needs: where it goes and what it
/// says.
#[derive(Debug, Clone)]
pub struct AuditDelivery {
    log: ClusterAuditLog,
    log_group: String,
    log_stream: String,
    event: ::serde_json::Value,
    at: SystemTime,
}

impl AuditDelivery {
    /// Deliver the record, through the ONE delivery sequence this service exports a
    /// control plane log record with ([`super::deliver_log_record`]): the cluster's log
    /// group and its audit stream are created if CloudWatch Logs does not hold them
    /// yet, then the one event is put.
    ///
    /// Best effort and never fatal — see this module's header. Call it with NO store
    /// guard held: it awaits.
    pub async fn deliver(self) {
        // No Region label, so no endpoint to reach CloudWatch Logs at: the same
        // silence an unmounted CloudWatch Logs leaves.
        let Some(endpoint) = super::peer_endpoint("logs", &self.log.region) else {
            return;
        };
        let logs = callback_client!(
            aws_sdk_cloudwatchlogs,
            callback = &self.log.callback,
            endpoint,
            self.log.region
        );
        super::deliver_log_record(
            &logs,
            &self.log.cluster_name,
            &self.log_group,
            &self.log_stream,
            self.event.to_string(),
            self.at,
        )
        .await;
    }
}
