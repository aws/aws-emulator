// SPDX-License-Identifier: CC0-1.0

//! The NOTIFICATIONS plugin's channel API on a domain's data plane:
//! `/_plugins/_notifications/configs`, where a client creates the channel a monitor's action
//! sends through and reads back the channels the cluster holds.
//!
//! Every answer is computed from the records the create and the update persisted
//! ([`crate::state::notifications`]): the list reports the configurations the domain holds,
//! in the order its store keeps them, and a read of one reports that one. Nothing is
//! invented for a channel nobody created.

use std::time::SystemTime;

use aws_smithy_runtime_api::http::Response as RtResponse;
use serde_json::{json, Value};

use crate::state::alerting::new_plugin_record_id;
use crate::state::notifications::{
    config_list, NotificationConfig, CONFIG_ID_MEMBER, CONFIG_MEMBER, CONFIG_TYPE_MEMBER,
    NOTIFICATION_CONFIG_QUOTA,
};
use crate::state::{fgac, ingest, Domain};

use super::{error, guarded, ok, wrong_method, Request, STATUS_EXCEPTION};

/// `POST /_plugins/_notifications/configs` (create) and `GET` (list).
pub(super) fn configs_request(
    domain: &mut Domain,
    request: &Request<'_>,
    now: SystemTime,
) -> RtResponse {
    match request.method {
        "POST" => guarded(
            domain,
            request,
            fgac::ACTION_CREATE_NOTIFICATION_CONFIG,
            |domain| create_config(domain, request, now),
        ),
        "GET" => guarded(
            domain,
            request,
            fgac::ACTION_GET_NOTIFICATION_CONFIG,
            |domain| ok(&listed_configs(domain, request)),
        ),
        _ => wrong_method(request, "POST, GET"),
    }
}

/// `GET`, `PUT` and `DELETE /_plugins/_notifications/configs/{config_id}`.
pub(super) fn config_request(
    domain: &mut Domain,
    request: &Request<'_>,
    config_id: &str,
    now: SystemTime,
) -> RtResponse {
    match request.method {
        "GET" => guarded(
            domain,
            request,
            fgac::ACTION_GET_NOTIFICATION_CONFIG,
            |domain| match domain.notification_configs.get(config_id) {
                Some(held) => ok(&config_list(&[held])),
                None => config_not_found(config_id),
            },
        ),
        "PUT" => guarded(
            domain,
            request,
            fgac::ACTION_UPDATE_NOTIFICATION_CONFIG,
            |domain| update_config(domain, request, config_id, now),
        ),
        "DELETE" => guarded(
            domain,
            request,
            fgac::ACTION_DELETE_NOTIFICATION_CONFIG,
            |domain| match domain.notification_configs.remove(config_id) {
                Some(_removed) => ok(&json!({
                    "delete_response_list": { config_id: "OK" },
                })),
                None => config_not_found(config_id),
            },
        ),
        _ => wrong_method(request, "GET, PUT, DELETE"),
    }
}

/// The configurations a list reports: every one the domain holds, narrowed to the ids the
/// request named where it named any (`config_id`, which the plugin's own list takes as a
/// filter).
fn listed_configs(domain: &Domain, request: &Request<'_>) -> Value {
    let named = request.parameter(CONFIG_ID_MEMBER);
    let selected: Vec<&NotificationConfig> = domain
        .notification_configs
        .values()
        .filter(|held| {
            named
                .as_deref()
                .is_none_or(|named| named.split(',').any(|id| id == held.config_id))
        })
        .collect();
    config_list(&selected)
}

/// Create one channel configuration.
fn create_config(domain: &mut Domain, request: &Request<'_>, now: SystemTime) -> RtResponse {
    let body = match request.json() {
        Ok(body) => body.unwrap_or(Value::Null),
        Err(refusal) => return *refusal,
    };
    let config = match submitted_config(&body) {
        Ok(config) => config,
        Err(refusal) => return *refusal,
    };
    // The caller may name the id itself — "config_id (optional): the id of the created
    // configuration" — and the plugin mints one for a request that names none.
    let config_id = match body.get(CONFIG_ID_MEMBER).and_then(Value::as_str) {
        Some(named) if !named.is_empty() => named.to_string(),
        _ => new_plugin_record_id(),
    };
    if domain.notification_configs.contains_key(&config_id) {
        return error(
            409,
            STATUS_EXCEPTION,
            &format!("A config with config_id {config_id} already exists"),
        );
    }
    if domain.notification_configs.len() >= NOTIFICATION_CONFIG_QUOTA {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "cluster [{}] already holds the maximum of {NOTIFICATION_CONFIG_QUOTA} \
                 notification channel configurations, so config [{config_id}] could not be \
                 created",
                super::cluster_name(domain)
            ),
        );
    }
    domain.notification_configs.insert(
        config_id.clone(),
        NotificationConfig {
            config_id: config_id.clone(),
            config,
            created_at: now,
            updated_at: now,
        },
    );
    ok(&json!({ CONFIG_ID_MEMBER: config_id }))
}

/// Replace the configuration the path names with the one the request carries.
fn update_config(
    domain: &mut Domain,
    request: &Request<'_>,
    config_id: &str,
    now: SystemTime,
) -> RtResponse {
    let body = match request.json() {
        Ok(body) => body.unwrap_or(Value::Null),
        Err(refusal) => return *refusal,
    };
    let config = match submitted_config(&body) {
        Ok(config) => config,
        Err(refusal) => return *refusal,
    };
    let Some(held) = domain.notification_configs.get_mut(config_id) else {
        return config_not_found(config_id);
    };
    held.config = config;
    held.updated_at = now;
    ok(&json!({ CONFIG_ID_MEMBER: config_id }))
}

/// The `config` object the request carries, or the refusal for a request carrying none.
///
/// Two things are required of it, and nothing else: it is an object, and it names the
/// channel's own type. Everything else a channel carries belongs to the CHANNEL KIND the
/// type names — the topic of an SNS channel, the URL of a webhook — so it is stored as the
/// caller wrote it and read by whatever sends through that channel.
fn submitted_config(body: &Value) -> Result<Value, Box<RtResponse>> {
    let Some(config) = body.get(CONFIG_MEMBER).filter(|config| config.is_object()) else {
        return Err(Box::new(error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!("[{CONFIG_MEMBER}] required property is missing"),
        )));
    };
    if config
        .get(CONFIG_TYPE_MEMBER)
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(Box::new(error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!("[{CONFIG_TYPE_MEMBER}] required property is missing"),
        )));
    }
    Ok(config.clone())
}

/// The refusal for a channel configuration the domain does not hold.
fn config_not_found(config_id: &str) -> RtResponse {
    error(
        404,
        STATUS_EXCEPTION,
        &format!("NotificationConfig {config_id} not found"),
    )
}
