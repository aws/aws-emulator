// SPDX-License-Identifier: CC0-1.0

//! The FAILOVER SLATE configurations this service holds: the resource
//! `CreateFailoverSlateConfiguration` mints, the two reads report, `Update…` changes and
//! `Delete…` removes, plus the rules more than one of those five operations applies.
//!
//! RULE: a failover slate configuration is a plain configuration resource of this
//! service — minted under its own ARN, reported from what was persisted, and referenced
//! by a channel through `Channel$failoverSlateConfigurationArn` — so everything here is
//! the same shape as the ad- and recording-configuration rules beside it: one `BTreeMap`
//! keyed by ARN for one stable order, a cap on caller-driven growth, and the documented
//! default of every member a request may omit.
//!
//! The one thing it has that an ad configuration does not is a STATE: the modelled
//! `FailoverSlateConfigurationState` is `CREATING | CREATE_FAILED | CREATED | UPDATING`
//! (`smithy_query(query="shape",shape="FailoverSlateConfigurationState")`), so a create
//! answers `CREATING` and an update `UPDATING`, and the move to `CREATED` is owed by the
//! CLOCK rather than by anything a caller does — exactly as a recording configuration's
//! `CREATING` → `ACTIVE` is (`settle_recording_configurations` on the state struct). It is
//! materialised by [`settle_failover_slate_configurations`], which every operation that
//! reports one of these resources runs first, so a read only ever materialises what was
//! already due.

use super::*;

/// A persisted failover slate configuration together with the instant at which the
/// transition the clock owes it — `CREATING` or `UPDATING` to `CREATED` — becomes due.
#[derive(Debug, Clone)]
pub struct FailoverSlateConfigurationRecord {
    /// What every read of this resource answers.
    pub configuration: FailoverSlateConfiguration,
    /// When the owed transition falls due; see [`FAILOVER_SLATE_CONFIGURATION_SETTLEMENT`].
    pub settled_at: SystemTime,
}

/// The state a failover slate configuration is minted in, from the modelled
/// `FailoverSlateConfigurationState` values.
pub const FAILOVER_SLATE_CONFIGURATION_CREATING: &str = "CREATING";

/// The state an update puts it in while the change is being applied, from the same
/// modelled values.
pub const FAILOVER_SLATE_CONFIGURATION_UPDATING: &str = "UPDATING";

/// The state it settles in once the clock owes the move: the terminal, ready state of the
/// modelled enumeration.
pub const FAILOVER_SLATE_CONFIGURATION_CREATED: &str = "CREATED";

/// How long after the call that started it the move to [`FAILOVER_SLATE_CONFIGURATION_CREATED`]
/// falls due.
///
/// AWS publishes no duration for it, so this is the emulator's own span for a transition
/// the clock owes. It is deliberately non-zero: a caller that reads straight after the
/// create still observes `CREATING`, exactly as it would against AWS.
pub const FAILOVER_SLATE_CONFIGURATION_SETTLEMENT: Duration = Duration::from_secs(3);

/// The ceiling on how many failover slate configurations this store holds. See
/// [`super::MAX_CHANNELS`] for why a cap exists at all: a create is the only thing that
/// adds one, it never expires, and past the cap the operation answers the
/// `ServiceQuotaExceededException` (HTTP 402) IVS models on it. The number is the
/// emulator's own bound, not a published AWS default.
pub const MAX_FAILOVER_SLATE_CONFIGURATIONS: usize = 1_000;

/// The name the failover-slate-configuration quota's refusal states — the quota is on the
/// resource, so the message names the resource type rather than the operation. See
/// [`super::quota_exceeded_message`].
pub const FAILOVER_SLATE_CONFIGURATION_QUOTA_RESOURCE: &str = "failover-slate-configuration";

/// The stamp a `ListFailoverSlateConfigurations` page token carries, so a token issued
/// for one list cannot be replayed against another. See [`super::CHANNEL_PAGE`].
pub const FAILOVER_SLATE_CONFIGURATION_PAGE: &str = "failover-slate-configuration";

/// The resource segment a `FailoverSlateConfigurationArn` carries, from the modelled
/// pattern `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:failover-slate-configuration/[a-zA-Z0-9-]+$`
/// (`smithy_query(query="shape",shape="FailoverSlateConfigurationArn")`). See
/// [`super::CHANNEL_RESOURCE_TYPE`].
pub const FAILOVER_SLATE_CONFIGURATION_RESOURCE_TYPE: &str = "failover-slate-configuration";

/// How the kind is SPELLED in the conflict a delete answers while a channel still names
/// the configuration, the way the ad-configuration delete spells its own kind. See
/// [`super::resource_referenced_message`].
pub const FAILOVER_SLATE_CONFIGURATION_KIND: &str = "failover slate configuration";

/// A `FailoverSlateConfigurationArn`:
/// `arn:aws:ivs:<region>:<account>:failover-slate-configuration/<id>`.
pub fn failover_slate_configuration_arn(region: &str, id: &str) -> String {
    resource_arn(FAILOVER_SLATE_CONFIGURATION_RESOURCE_TYPE, region, id)
}

/// The `activationThresholdSeconds` a configuration reports when the request named none:
/// "Number of seconds of ingest interruption before the failover slate activates.
/// Default: `3`" (the model doc on
/// `FailoverSlateConfiguration$activationThresholdSeconds`).
pub const FAILOVER_SLATE_ACTIVATION_THRESHOLD_SECONDS_DEFAULT: i32 = 3;

/// The `minSlateDisplaySeconds` default: "Minimum number of seconds that the failover
/// slate displays after activation. Default: `3`" (the model doc on that member).
pub const FAILOVER_SLATE_MIN_DISPLAY_SECONDS_DEFAULT: i32 = 3;

/// The `maxSlateDisplaySeconds` default: "Maximum number of seconds that the failover
/// slate displays. Default: `60`" (the model doc on that member).
pub const FAILOVER_SLATE_MAX_DISPLAY_SECONDS_DEFAULT: i32 = 60;

/// Materialise every `CREATING`/`UPDATING` → `CREATED` transition the clock already owes.
///
/// A configuration is minted in `CREATING` and an update puts it in `UPDATING`; AWS turns
/// it `CREATED` on its own, so the move is owed at
/// [`FailoverSlateConfigurationRecord::settled_at`] whether or not anybody calls. Every
/// operation that reports one of these resources runs this first, so a read materialises
/// only what was already due and never advances a resource because it was looked at.
pub fn settle_failover_slate_configurations(
    state: &mut ComAmazonawsIvsAmazoninteractivevideoserviceState,
) {
    let now = SystemTime::now();
    for record in state.failover_slate_configurations.values_mut() {
        let settling = record.configuration.state == FAILOVER_SLATE_CONFIGURATION_CREATING
            || record.configuration.state == FAILOVER_SLATE_CONFIGURATION_UPDATING;
        if settling && record.settled_at <= now {
            record.configuration.state = FAILOVER_SLATE_CONFIGURATION_CREATED.to_string();
        }
    }
}

/// The failover slate configuration the channel `channel_arn` names has associated, as
/// this store holds it.
///
/// A channel's association is the ARN it carries in `failoverSlateConfigurationArn`, so
/// the configuration itself is that ARN RESOLVED against the configurations this store
/// holds — the same reading as [`channel_ad_configuration`]. A channel that names none
/// (the member absent, or the empty string a create persists for a caller that sent
/// nothing), and one whose member names a configuration this store does not hold, both
/// have none associated.
pub fn channel_failover_slate_configuration<'a>(
    state: &'a ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
) -> Option<&'a FailoverSlateConfiguration> {
    let arn = state
        .channels
        .get(channel_arn)?
        .failover_slate_configuration_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())?;
    state
        .failover_slate_configurations
        .get(arn)
        .map(|record| &record.configuration)
}

/// The `FailoverSlateConfigurationSummary` projection of a persisted configuration: every
/// member that shape declares, read off the stored resource. The summary and the detail
/// shape declare the same members
/// (`smithy_query(query="shape",shape="FailoverSlateConfiguration,FailoverSlateConfigurationSummary")`),
/// so the projection is the whole resource rather than a subset of it.
pub fn failover_slate_configuration_summary(
    configuration: &FailoverSlateConfiguration,
) -> FailoverSlateConfigurationSummary {
    FailoverSlateConfigurationSummary {
        arn: configuration.arn.clone(),
        name: configuration.name.clone(),
        activation_threshold_seconds: configuration.activation_threshold_seconds,
        min_slate_display_seconds: configuration.min_slate_display_seconds,
        max_slate_display_seconds: configuration.max_slate_display_seconds,
        state: configuration.state.clone(),
        tags: configuration.tags.clone(),
    }
}
