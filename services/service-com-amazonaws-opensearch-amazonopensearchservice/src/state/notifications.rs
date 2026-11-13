// SPDX-License-Identifier: CC0-1.0

//! The NOTIFICATIONS plugin's channel configurations a domain holds: the records
//! `POST /_plugins/_notifications/configs` writes, and what each of them says about where a
//! notification goes.
//!
//! A channel configuration is the plugin's own resource — "a notification channel is a
//! destination … such as Amazon SNS, Amazon Chime, Slack or a custom webhook" — addressed by
//! its `config_id` and reported back by the plugin's own list and get. This module holds one
//! such record and reads the two things the rest of the emulator asks of it: whether it is
//! ENABLED, and WHERE it sends.
//!
//! Only the Amazon SNS channel is a destination this emulator can really deliver to: an SNS
//! topic is a resource of another emulated service in the same process, so a notification to
//! it is published with `sns:Publish` and fans out to that topic's own subscriptions
//! ([`super::notification_publication`]). A configuration of any other `config_type` is
//! stored and reported back as written — the plugin's own records are the caller's — and a
//! notification addressed to one is reported as the action failure it is, since nothing in
//! this process can reach a Chime room, a Slack workspace or a webhook host.

use std::time::SystemTime;

use ::serde_json::{json, Value};

/// How many channel configurations one domain may hold before a create is refused — the
/// same kind of bound [`super::ISM_POLICY_QUOTA`] is, for the same reason: a caller looping
/// with fresh names must not grow this process's state without end.
pub const NOTIFICATION_CONFIG_QUOTA: usize = 100;

/// The `config_type` an Amazon SNS channel carries.
pub const SNS_CONFIG_TYPE: &str = "sns";

/// The member a channel's type is named in.
pub const CONFIG_TYPE_MEMBER: &str = "config_type";

/// The member the whole configuration arrives and is reported under.
pub const CONFIG_MEMBER: &str = "config";

/// The member a create may name the configuration's own id in.
pub const CONFIG_ID_MEMBER: &str = "config_id";

/// The member an SNS channel names its topic in.
pub const TOPIC_ARN_MEMBER: &str = "topic_arn";

/// The member every channel carries its own name in.
pub const NAME_MEMBER: &str = "name";

/// The member a channel carries its enabled flag in.
pub const IS_ENABLED_MEMBER: &str = "is_enabled";

/// One channel configuration of a domain, as the notifications plugin holds it.
///
/// The `config` object is stored AS WRITTEN — the plugin reports back the configuration the
/// caller sent — with the two instants the plugin stamps itself beside it.
#[derive(Debug, Clone)]
pub struct NotificationConfig {
    /// The configuration's own id, unique on the cluster, so the id IS the key it is held
    /// under.
    pub config_id: String,
    /// The `config` object as it was written.
    pub config: Value,
    /// When the configuration was created — what `created_time_ms` reports.
    pub created_at: SystemTime,
    /// When it was last written — what `last_updated_time_ms` reports.
    pub updated_at: SystemTime,
}

impl NotificationConfig {
    /// The channel's type, as the configuration names it.
    #[must_use]
    pub fn config_type(&self) -> &str {
        self.config
            .get(CONFIG_TYPE_MEMBER)
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The channel's own name.
    #[must_use]
    pub fn name(&self) -> &str {
        self.config
            .get(NAME_MEMBER)
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// Whether the channel is enabled. "is_enabled — whether the channel is available for
    /// sending notifications"; a configuration that names none is available, which is the
    /// plugin's own default for the flag.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.config
            .get(IS_ENABLED_MEMBER)
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    /// The Amazon SNS topic this channel publishes to, or `None` when it is not an SNS
    /// channel or names no topic.
    #[must_use]
    pub fn sns_topic_arn(&self) -> Option<&str> {
        if self.config_type() != SNS_CONFIG_TYPE {
            return None;
        }
        self.config
            .get(SNS_CONFIG_TYPE)?
            .get(TOPIC_ARN_MEMBER)?
            .as_str()
            .filter(|arn| !arn.is_empty())
    }

    /// This configuration as the plugin's own get and list report one: the id, the two
    /// instants it stamped, and the configuration as written.
    #[must_use]
    pub fn view(&self) -> Value {
        json!({
            CONFIG_ID_MEMBER: self.config_id,
            "last_updated_time_ms": super::epoch_millis(self.updated_at),
            "created_time_ms": super::epoch_millis(self.created_at),
            CONFIG_MEMBER: self.config.clone(),
        })
    }
}

/// The envelope the plugin reports a set of configurations in — the same one its get by id
/// and its list answer, since both report a page of the configurations the cluster holds.
#[must_use]
pub fn config_list(configs: &[&NotificationConfig]) -> Value {
    json!({
        "start_index": 0,
        "total_hits": configs.len(),
        "total_hit_relation": "eq",
        "config_list": configs.iter().map(|config| config.view()).collect::<Vec<Value>>(),
    })
}
