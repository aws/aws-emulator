// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-medialive-medialive service.
//!
//! The store is PARTITIONED BY REGION
//! ([`ComAmazonawsMedialiveMedialiveStore`]): MediaLive is a regional service,
//! so every resource it mints belongs to the region whose endpoint created it —
//! the region its ARN names — and the state an operation is handed
//! ([`service_state`]) is the partition of the region ITS request was signed
//! for. Everything below therefore describes ONE region's resources, and a
//! reader, a mutation or a reference that resolves an id resolves it in that
//! region alone.
//!
//! The channel lifecycle lives here: one [`ChannelRecord`] per created channel,
//! the monotonic id counter that mints its `Id`, and the idempotency-token map
//! that makes a retried `CreateChannel` return the channel it already created.
//! The input lifecycle lives beside it: one [`InputRecord`] per created input,
//! its own id counter and its own token map. The SDI-source resource lives here
//! too: one [`SdiSourceRecord`] per created source, its own id counter and its
//! own token map. So do signal maps: one [`SignalMapRecord`] per created signal
//! map, carrying the media-resource graph discovery found by walking the
//! channels this store already holds, and the latest monitor deployment
//! ([`MonitorDeploymentRecord`]) beside the latest one that succeeded, which is
//! how both the signal map's `MonitorDeployment` and its summary's
//! `MonitorDeploymentStatus` are reported out of state. A deployment is of one
//! of the three kinds its [`MonitorDeploymentKind`] names: the real deployment
//! and the dry run `StartMonitorDeployment` starts, and the deletion of the
//! monitor `StartDeleteMonitorDeployment` starts — one record either way, so a
//! signal map has exactly one latest monitor deployment whatever started it.
//!
//! The MediaLive Anywhere network lives here too: one [`NetworkRecord`] per
//! created network, its own id counter and its own token map. A network's
//! `State` is the one lifecycle member it owes the clock (`CREATING` until the
//! creation comes due, then `ACTIVE`; `UPDATING` until an accepted settings
//! change comes due, then `ACTIVE` again; `DELETING` until an accepted delete
//! comes due, then the terminal `DELETED`) and its `AssociatedClusterIds` are derived
//! from the rest of the state on every read, never recorded at create time.
//!
//! The INPUT DEVICES this account has claimed live here as well: one
//! [`InputDeviceRecord`] per claimed device, keyed by the device id the claim
//! named rather than by an id this service mints, because a Link unit's id is
//! its own. A device's `DeviceUpdateStatus` comes out of the maintenance window
//! `StartInputDeviceMaintenanceWindow` opens on it, its `InputDeviceState` out
//! of the `StartInputDevice` / `StopInputDevice` commands it has accepted, and
//! its `OutputType` and
//! `MedialiveInputArns` out of the inputs this store holds — see
//! [`input_devices`]. A device also carries the TRANSFER pending on it, which
//! `TransferInputDevice` records, `ListInputDeviceTransfers` reports in the
//! direction the reader asks for, `AcceptInputDeviceTransfer` ends for the
//! account the transfer names as its recipient, `RejectInputDeviceTransfer`
//! ends for that same recipient when it refuses the device instead, and
//! `CancelInputDeviceTransfer`
//! ends for the account that requested it, and the latest THUMBNAIL it has
//! sent, which `DescribeInputDeviceThumbnail` reports — one only a connected
//! Link unit sends, so a device claimed through this emulator's API has none.
//!
//! The input security group lifecycle lives here too: one
//! [`InputSecurityGroupRecord`] per created group and the counter that mints its
//! `Id`. A group's `Inputs` and `Channels` are derived from the rest of the
//! state on every read, never recorded at create time, and so is its `State` —
//! out of what uses the group and out of whether `DeleteInputSecurityGroup` has
//! deleted it.
//!
//! The MediaLive Anywhere NODE lives in its own module ([`nodes`]): one
//! [`NodeRecord`] per created node, keyed by its minted `Id` and carrying the
//! cluster the create was addressed to, because a node's id is "Unique in the
//! Cluster" and every node operation names both. Its deletion is a
//! DEREGISTRATION — the state its enum models — so a deleted node ends in
//! `DEREGISTERED` rather than in a `DELETED` the model does not declare.
//!
//! A delete never drops a record: a deleted channel, cluster, input, input
//! security group, network or SdiSource keeps its record in the terminal
//! `DELETED` state its own
//! state enum models, so a reader goes on reporting it there (which is what the
//! SDK's `ChannelDeleted` / `InputDeleted` waiters poll for) instead of losing
//! it. The `DELETING` step a channel's, a cluster's, an input's and a network's
//! enum models is
//! what the clock owes between the call and that terminal state.
//!
//! ACCEPTING the delete is what RELEASES everything the resource held, so from
//! that instant it is no longer *currently using* anything and it leaves the
//! derived `Inputs`, `Channels`, `AttachedChannels`, `MedialiveInputArns`,
//! `ChannelIds` and `AssociatedClusterIds` lists — the transient `DELETING`
//! step included, because a resource being torn down ingests through nothing
//! and holds nothing any more. That accepted delete is the ONE cut point every
//! one of those lists applies, and so the `State`, the output type and the
//! "in use" refusals derived from one of them fall back to unused at the same
//! instant rather than one transient span later.
//!
//! A kept record is READABLE, not writable: once a delete has been accepted for
//! an id, that id names no resource a caller may still change. Every MUTATION
//! therefore resolves its id through the live lookups
//! ([`ComAmazonawsMedialiveMedialiveState::live_input`],
//! [`ComAmazonawsMedialiveMedialiveState::live_cluster`],
//! [`ComAmazonawsMedialiveMedialiveState::mutable_channel`] over
//! [`ComAmazonawsMedialiveMedialiveState::live_channel_mut`], and the tag
//! writers' own private `resource_tags_mut`) and answers a dead id with its own
//! modelled not-found error. That resolution comes FIRST, before the mutation
//! looks at the resource's state at all, so a tombstone is answered as not found
//! and never with the state conflict a resource that still exists gets, while a
//! read goes on reporting the record in its terminal state.
//!
//! The MULTIPLEX is the resource whose own recorded behaviour differs, so its
//! mutations are NOT written over that lookup: its record stays RESOLVABLE after
//! the delete is accepted, and the real service goes on judging a call against
//! the state that record is in. In the
//! `medialive.multiplex-start-without-programs-rejected` fixture the multiplex is
//! already gone from `ListMultiplexes` while a `StartMultiplex` sent straight
//! afterwards is refused with the state conflict naming its `DELETING` state, not
//! with a not-found answer. So `StartMultiplex`, `UpdateMultiplex` and
//! `DeleteMultiplex` resolve their `MultiplexId` across the whole
//! [`ComAmazonawsMedialiveMedialiveState::multiplexes`] store and answer the
//! conflict their own state rule makes, keeping their modelled not-found error
//! for an id this account holds no record for at all.
//!
//! The INPUT SECURITY GROUP is the one resource here whose delete is a SOFT
//! delete: it only drops the group from `ListInputSecurityGroups`, leaving the
//! group individually addressable AND still mutable. So a mutation of a group
//! resolves its id across the whole
//! [`ComAmazonawsMedialiveMedialiveState::input_security_groups`] store and
//! refuses only an id this account never held.
//! [`ComAmazonawsMedialiveMedialiveState::live_input_security_group`] is what a
//! request REFERENCING a group resolves through instead: changing a deleted
//! group is allowed, taking a new dependency on one is not.
//!
//! A channel's schedule lives on its own [`ChannelRecord`], as the
//! [`ChannelSchedule`] of the [`schedules`] slice: the actions
//! `BatchUpdateSchedule` created and `DeleteSchedule` clears, swept of the ones
//! whose start time is over an hour past whenever [`ComAmazonawsMedialiveMedialiveState::settle`]
//! runs.
//!
//! A channel's ALERTS live on its own [`ChannelRecord`] too, as the
//! [`ChannelAlerts`] log of the [`channel_alerts`] slice: the alerts the service
//! has observed on that channel, which `ListAlerts` reports. Nothing in this
//! emulator ingests media, so no operation observes an alert condition and the
//! log stays empty unless [`ChannelRecord::raise_alert`] is called — a read
//! reports what the log holds for the pipelines the channel's class gives it
//! ([`ChannelRecord::reportable_alerts`]) and never invents an alert for a
//! channel that is merely running.
//!
//! The monitor side of the service lives here as well: one
//! [`CloudWatchAlarmTemplateGroupRecord`] per created cloudwatch alarm template
//! group and one [`EventBridgeRuleTemplateGroupRecord`] per created eventbridge
//! rule template group, each keyed by its minted `Id` and carrying the region it
//! was created in, so a group's name is unique "within the scope of a resource
//! type in a specific region" (the `Name` member's own documentation). The
//! TEMPLATES those groups hold live here too: one
//! [`CloudWatchAlarmTemplateRecord`] per created cloudwatch alarm template and
//! one [`EventBridgeRuleTemplateRecord`] per created eventbridge rule template,
//! each carrying the id of the group its `GroupIdentifier` resolved to, so a
//! template reports the group it belongs to out of state rather than out of the
//! request.
//!
//! The ACCOUNT's own MediaLive settings live here too, beside the region
//! partitions rather than inside one:
//! one [`AccountConfigurationRecord`] per region an `UpdateAccountConfiguration`
//! has written one in. That is not a resource a caller creates — the account
//! always has a configuration — so a region nothing has configured yet reads as
//! the unset configuration rather than as missing. See the
//! [`account_configuration`] slice.
//!
//! Every value a channel-, input-, input-security-group-, SdiSource-,
//! signal-map- or template-group-shaped response carries is either stored
//! verbatim from the request that created the resource or derived from the stored
//! state ([`ComAmazonawsMedialiveMedialiveState::channel_description`],
//! [`ComAmazonawsMedialiveMedialiveState::input_description`],
//! [`ComAmazonawsMedialiveMedialiveState::sdi_source_description`],
//! [`ComAmazonawsMedialiveMedialiveState::signal_map_description`],
//! [`alarm_template_group_output`],
//! [`alarm_template_output`],
//! [`event_bridge_rule_template_group_output`]) - no operation builds a response
//! out of its own request.
//!
//! MediaLive Anywhere's CLUSTER lives here too: one [`ClusterRecord`] per
//! created cluster, its own id counter and its own token map, carrying the
//! alerts raised against that cluster. A cluster's `ChannelIds` is derived from
//! the channels this store already holds, never recorded at create time, and so
//! is its `State` — out of the `CREATING` -> `ACTIVE` transition the clock owes
//! it.
//!
//! The CHANNEL PLACEMENT GROUPS those clusters hold live here as well: one
//! [`ChannelPlacementGroupRecord`] per created group, its own id counter and its
//! own token map, each carrying the id of the cluster it was created in, so the
//! (`ClusterId`, `ChannelPlacementGroupId`) pair every one of its operations
//! names it by resolves out of state. A group's `Channels` is derived from the
//! channels this store already holds — the ones whose
//! `AnywhereSettings$ChannelPlacementGroupId` names the group — and its `State`
//! from the node assignment the clock owes it
//! (`ASSIGNING` -> `ASSIGNED`, `UNASSIGNING` -> `UNASSIGNED`).
//!
//! Tags are NOT a store of their own: every resource record above carries its
//! own tag map, and the tag operations reach the map of the resource their
//! `ResourceArn` names through
//! [`ComAmazonawsMedialiveMedialiveState::resource_tags`] (reading),
//! [`ComAmazonawsMedialiveMedialiveState::insert_resource_tags`] and
//! [`ComAmazonawsMedialiveMedialiveState::remove_resource_tags`] (writing), so a
//! tag written by `CreateTags` is the tag that resource's own describe reports.
//! The two writers own the per-resource tag bounds ([`MAX_RESOURCE_TAGS`],
//! [`MAX_TAG_KEY_LEN`], [`MAX_TAG_VALUE_LEN`]) and the reserved key namespace
//! ([`RESERVED_TAG_KEY_PREFIX`]), which is why the map itself is only ever
//! handed out mutably inside this module. An operation that stores a tag map a
//! request carried without going through them (a create, an update) refuses the
//! reserved namespace with [`reserved_tag_key_in`] before it writes.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AnywhereSettings, CdiInputSpecification, Channel, ChannelEgressEndpoint, ChannelSummary,
    DescribeAnywhereSettings, DescribeFollowerChannelSettings, DescribeInferenceSettings,
    DescribeLinkedChannelSettings, DescribeNetworkSummary, DescribePrimaryChannelSettings,
    EncoderSettings, EventBridgeRuleTemplateGroupSummary,
    InferenceSettings, Input, InputAttachment, InputDestination, InputDestinationRequest,
    InputDestinationRoute, InputDeviceSettings, InputSecurityGroup, InputSource,
    InputSourceRequest, InputSpecification, InputVpcRequest, InputWhitelistRule,
    InputWhitelistRuleCidr, IpPool,
    LinkedChannelSettings, MaintenanceCreateSettings, MaintenanceStatus, MediaConnectFlow,
    MediaConnectFlowRequest,
    MediaResource, MediaResourceNeighbor, MonitorDeployment, MulticastSettings,
    MulticastSettingsCreateRequest, MulticastSource, OutputDestination, PipelineDetail,
    Route,
    RouterDestination, RouterInputSettings, RouterSettings, SdiSource, SdiSourceSummary,
    Smpte2110ReceiverGroupSettings, SignalMapSummary, SpecialRouterSettings,
    SrtCallerDecryption, SrtCallerSource, SrtListenerDecryption, SrtListenerSettings,
    SrtSettings, SrtSettingsRequest, SuccessfulMonitorDeployment,
    ValidationError, VpcOutputSettings, VpcOutputSettingsDescription,
};

/// The cloudwatch alarm template group slice: its record, its bounds, its
/// refusal, the store's methods and its output projection, in one module of its
/// own so the slice can be read (and merged) on its own. What stays in this file
/// is the part the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`] and the token key both group
/// resources share.
pub mod alarm_template_groups;

/// The channel schedule slice: one channel's schedule, the rules
/// `BatchUpdateSchedule` validates a request against, the clock-owed cleanup of
/// actions whose start time has passed and the start-time order a read reports.
/// Its store is the `schedule` member of [`ChannelRecord`] below, so a schedule
/// goes away with the channel that owns it.
pub mod schedules;

/// The channel CONFIGURATION slice: the one rule the operations that WRITE a
/// channel's configuration (`CreateChannel`, `UpdateChannel`) validate it
/// against before anything is stored, and the per-element problems a
/// configuration this service could not run is refused with.
pub mod channel_configurations;

pub use channel_configurations::ChannelConfiguration;

/// The IN-USE slice: the one rule a write that would hand a resource to a second
/// holder, or take one out from under its holder, is refused by — an input is
/// attached to at most one channel at a time, and an input security group is not
/// deleted while inputs still use it — plus the messages those refusals carry.
pub mod in_use_conflicts;

pub use in_use_conflicts::{input_already_attached_message, input_security_group_in_use_message};

/// The channel ALERT slice: one channel's alert log, the state an alert is in,
/// the `StateFilter` a read narrows it with and the set-time order a read
/// reports it in. Its store is the `alerts` member of [`ChannelRecord`] below,
/// for the same reason as the schedule: an alert belongs to exactly one channel.
pub mod channel_alerts;

pub use channel_alerts::{
    channel_alert_output, channel_alert_state, ChannelAlertRecord, ChannelAlerts,
    CHANNEL_ALERT_FILTERS, CHANNEL_ALERT_FILTER_ALL, MAX_CHANNEL_ALERTS,
};

pub use alarm_template_groups::{
    AlarmTemplateGroupRefusal, CloudWatchAlarmTemplateGroupRecord,
    MAX_ALARM_TEMPLATE_GROUPS, MAX_ALARM_TEMPLATE_GROUP_TOKENS,
};

/// The ACCOUNT CONFIGURATION slice: the per-region record this account's
/// `UpdateAccountConfiguration` writes and `DescribeAccountConfiguration` reads,
/// the shape of KMS key ARN it accepts and the bound on the store, in one module
/// of its own for the same reason as [`alarm_template_groups`]. What stays in
/// this file is the part the rest of the state needs: the
/// `account_configurations` store member on
/// [`ComAmazonawsMedialiveMedialiveStore`].
pub mod account_configuration;

pub use account_configuration::{
    is_kms_key_arn, AccountConfigurationRecord, MAX_ACCOUNT_CONFIGURATION_REGIONS,
};

pub use schedules::{
    parse_action_time, AppliedSchedule, ChannelSchedule, ChannelScheduleScope, ScheduleActionRecord,
    ScheduleRefusal, MAX_SCHEDULE_ACTIONS, SCHEDULE_ACTION_RETENTION,
};

pub(crate) use alarm_template_groups::alarm_template_group_output;

/// The channel placement group slice: its record, its bounds, its refusals, the
/// store's methods and its output projection, in one module of its own for the
/// same reason as [`alarm_template_groups`]. What stays in this file is the part
/// the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`].
pub mod channel_placement_groups;

pub use channel_placement_groups::{
    channel_placement_group_state, too_many_nodes_message, unassociable_node_message,
    ChannelPlacementGroupChanges, ChannelPlacementGroupDescription, ChannelPlacementGroupRecord,
    ChannelPlacementGroupRefusal, ChannelPlacementGroupUpdateRefusal,
    MAX_CHANNEL_PLACEMENT_GROUPS, MAX_CHANNEL_PLACEMENT_GROUP_NODES,
    MAX_CHANNEL_PLACEMENT_GROUP_TOKENS,
};

pub(crate) use channel_placement_groups::channel_placement_group_output;

/// The cloudwatch alarm TEMPLATE slice: its record, its bounds, its refusal, the
/// store's methods and its output projection, in one module of its own for the
/// same reason as [`alarm_template_groups`]. What stays in this file is the part
/// the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`] and the per-region token key every
/// monitor template resource shares.
pub mod alarm_templates;

pub use alarm_templates::{
    alarm_template_summary, check_alarm_template_settings, template_scope, AlarmTemplateRefusal,
    AlarmTemplateSettings, AlarmTemplateUpdateRefusal, CloudWatchAlarmTemplateChanges,
    CloudWatchAlarmTemplateRecord, NewCloudWatchAlarmTemplate, MAX_ALARM_PERIOD_SECONDS,
    MAX_ALARM_TEMPLATES, MAX_ALARM_TEMPLATE_TOKENS, MAX_METRIC_NAME_LEN, MIN_ALARM_DATAPOINTS,
    MIN_ALARM_PERIOD_SECONDS,
};

pub(crate) use alarm_templates::alarm_template_output;

/// The AWS-PROVIDED monitor template catalog slice: the cloudwatch alarm
/// template groups the service publishes, the templates inside them and the
/// records a region's partition is seeded with, in one module of its own for the
/// same reason as the two slices above. What stays in this file is the part the
/// rest of the state needs: the seeding of a partition
/// ([`ComAmazonawsMedialiveMedialiveState::with_mints`]) and the predicate every
/// writer checks before it modifies a monitor template resource
/// ([`is_published_id`]).
pub mod managed_monitor_templates;

pub use managed_monitor_templates::{
    is_published_id, listing_position, published_groups, published_resource_message,
    published_templates, service_owned_arn, PublishedWriteRefusal, CATALOG_PUBLISHED_AT,
};

/// The multiplex slice: its record, its bounds, its refusals, the store's
/// methods and its output projection, in one module of its own for the same
/// reason as the alarm template group slice above. The PROGRAMS a multiplex
/// holds are part of that slice, and so are the ALERTS raised on it: both live
/// on the multiplex record that owns them. What stays in this file is the part
/// the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`].
pub mod multiplexes;

pub use multiplexes::{
    multiplex_alert_state, multiplex_alert_type, multiplex_not_deletable_message,
    multiplex_program_not_found_message, multiplex_state,
    MultiplexProgramDeleteRefusal, MultiplexProgramRecord, MultiplexProgramRefusal,
    MultiplexProgramSettingsRefusal, MultiplexProgramUpdateRefusal, MultiplexRecord,
    MultiplexRefusal, MultiplexSettingOutOfRange, MultiplexStartRefusal, MultiplexStopRefusal,
    MultiplexUpdateRefusal, MAX_MULTIPLEXES,
    MAX_MULTIPLEX_ALERTS,
    MAX_MULTIPLEX_PROGRAMS,
    MAX_MULTIPLEX_PROGRAM_TOKENS,
    MAX_MULTIPLEX_TOKENS, MAX_SERVICE_DESCRIPTOR_NAME_LEN, MULTIPLEX_ALERT_STATE_FILTERS,
    MULTIPLEX_ALERT_STATE_FILTER_ALL, MULTIPLEX_AVAILABILITY_ZONES,
};

pub(crate) use multiplexes::{multiplex_description_output, multiplex_program_description_output};

/// The NODE slice — the MediaLive Anywhere Node: its record, its bounds, its
/// refusals, the store's methods and its output projection, in one module of its
/// own for the same reason as the multiplex slice above. What stays in this file
/// is the part the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`] and the node's entry in the ARN
/// resolution the tag operations share ([`TaggedResource`]).
pub mod nodes;

pub use nodes::{
    card_number_out_of_range_message, node_not_idle_message, node_state,
    too_many_interface_mappings_message, too_many_sdi_source_mappings_message, NodeChanges,
    NodeDeleteRefusal, NodeRecord, NodeRefusal, NodeStateRefusal, NodeUpdateRefusal,
    FIRST_SDI_CARD_NUMBER, MAX_NODES, MAX_NODE_INTERFACE_MAPPINGS, MAX_NODE_SDI_SOURCE_MAPPINGS,
    MAX_NODE_TOKENS, NODE_CONNECTION_STATE, SDI_SOURCE_MAPPING_MEMBERS_MESSAGE,
};

pub(crate) use nodes::node_description_output;

/// The node REGISTRATION SCRIPT slice — the artifact
/// `CreateNodeRegistrationScript` issues for a cluster, in a module of its own
/// beside the [`nodes`] slice it registers nodes into. What stays in this file is
/// the part the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`].
pub mod node_registration_scripts;

pub use node_registration_scripts::{
    NewNodeRegistrationScript, NodeRegistrationScriptRecord, NodeRegistrationScriptRefusal,
    MAX_NODE_REGISTRATION_SCRIPTS, MAX_NODE_REGISTRATION_SCRIPT_TOKENS,
};


/// The eventbridge rule TEMPLATE slice: its record, its bounds, its refusal, the
/// store's methods and its output projection, in one module of its own for the
/// same reason as [`alarm_template_groups`]. What stays in this file is the part
/// the rest of the state needs: the store members on
/// [`ComAmazonawsMedialiveMedialiveState`] and the per-region token key every
/// monitor template resource shares.
pub mod event_bridge_rule_templates;

pub use event_bridge_rule_templates::{
    event_bridge_rule_template_summary, event_targets_from_request, is_event_target_arn,
    is_event_type,
    EventBridgeRuleTemplateChanges, EventBridgeRuleTemplateRecord,
    EventBridgeRuleTemplateRefusal, EventBridgeRuleTemplateUpdateRefusal,
    NewEventBridgeRuleTemplate, EVENT_BRIDGE_RULE_TEMPLATE_EVENT_TYPES,
    MAX_EVENT_BRIDGE_RULE_TEMPLATES, MAX_EVENT_BRIDGE_RULE_TEMPLATE_TARGETS,
    MAX_EVENT_BRIDGE_RULE_TEMPLATE_TOKENS, MAX_EVENT_TARGET_ARN_LEN,
};

pub(crate) use event_bridge_rule_templates::event_bridge_rule_template_output;

/// The alert vocabulary every alert surface of this service shares: the two
/// states an alert is in, the `StateFilter` values each listing documents, the
/// membership rule that refuses a value outside them and the sentence that
/// refusal carries, and the two derivations (`State` out of the cleared instant,
/// and which alerts a filter admits) the cluster slice and the multiplex slice
/// both delegate to.
pub mod alerts;

pub use alerts::{
    alert_admitted_by, alert_state, alert_state_filter, alert_state_of,
    undocumented_alert_state_filter_message, ALERT_STATE_FILTERS, ALERT_STATE_FILTER_ALL,
};

/// The input device slice: the record `ClaimDevice` registers, the device-id
/// form it accepts, the maintenance window
/// `StartInputDeviceMaintenanceWindow` opens and the projection every operation
/// that reports a device reads, in one module of its own for the same reason as
/// the slices above. What stays in this file is the part the rest of the state
/// needs: the store member on [`ComAmazonawsMedialiveMedialiveState`] and the
/// ARN resolution the tag operations share.
pub mod input_devices;

pub use input_devices::{
    configurable_settings_error, input_device_transfer_type, input_device_type_for_id,
    is_account_id, not_a_device_id_message, not_an_account_id_message, reboot_input_device_force,
    repeated_audio_channel_pair_message, settings_member_mismatch_message,
    InputDeviceConfiguration, InputDeviceRecord,
    InputDeviceThumbnailRecord, InputDeviceTransferRecord, TransferAcceptance, TransferCancellation,
    TransferRejection,
    FIRST_AUDIO_CHANNEL_PAIR_ID, LAST_AUDIO_CHANNEL_PAIR_ID, MAX_AUDIO_CHANNEL_PAIRS,
    MAX_INPUT_DEVICES,
    MAX_INPUT_DEVICE_THUMBNAIL_BYTES, MAX_TRANSFER_MESSAGE_CHARS,
};

pub(crate) use input_devices::input_device_output;

/// The cluster slice: MediaLive Anywhere's `Cluster` record, its bounds, its
/// refusals, the store's methods and its output projections, in one module of
/// its own for the same reason as the multiplex slice above. The ALERTS raised
/// against a cluster are part of that slice: they live on the cluster record
/// that owns them. What stays in this file is the part the rest of the state
/// needs: the store members on [`ComAmazonawsMedialiveMedialiveState`].
pub mod clusters;

pub use clusters::{
    cluster_alert_output, cluster_alert_state, cluster_not_idle_message, cluster_state,
    described_cluster_network_settings, mapped_network_member,
    too_many_cluster_interface_mappings_message, ClusterAlertRecord, ClusterChanges,
    ClusterDeleteRefusal, ClusterRecord, ClusterRefusal, ClusterUpdateRefusal, NewCluster,
    CLUSTER_ALERT_FILTERS, CLUSTER_ALERT_FILTER_ALL, MAX_CLUSTERS, MAX_CLUSTER_ALERTS,
    MAX_CLUSTER_INTERFACE_MAPPINGS, MAX_CLUSTER_TOKENS,
};

pub(crate) use clusters::cluster_description_output;

/// The thumbnail slice: the frames a channel's encoder pipelines captured, the
/// bound on that log, the derived reader `DescribeThumbnails` reports through
/// and the store's one writer, in one module of its own for the same reason as
/// the cluster slice above. Its store is the `thumbnails` member of
/// [`ChannelRecord`] below, so a capture log goes away with the channel that
/// owns it.
pub mod thumbnails;

pub use thumbnails::{
    thumbnail_detail_output, thumbnail_state, ThumbnailRecord, MAX_CHANNEL_THUMBNAILS,
};

/// The OFFERING slice: the catalog of reserved resources available for purchase
/// that `ListOfferings`, `DescribeOffering` and `PurchaseOffering` all answer
/// from, in one module of its own for the same reason as the slices above. It is
/// a catalog rather than a store — an offering is not a resource a caller
/// creates — so it adds no member to [`ComAmazonawsMedialiveMedialiveState`];
/// what it takes FROM the state is the configuration of a stored channel, which
/// is what the `ChannelConfiguration` filter matches offerings against.
pub mod offerings;

pub use offerings::{
    catalog, offering_by_id, offering_id, offering_index, offering_page,
    reservation_resource_type, specification_filter_refusal, ChannelOfferingConfiguration,
    OfferingFilters, OfferingSpec, OFFERING_CURRENCY_CODE, OFFERING_DURATION,
    OFFERING_DURATION_UNITS, OFFERING_FIXED_PRICE, OFFERING_TYPE,
};

/// The RESERVATION slice: the reserved resources the account has bought against
/// the [`offerings`] catalog, in one module of its own for the same reason as the
/// slices above. Its store is the [`ComAmazonawsMedialiveMedialiveState::reservations`]
/// member below, and every member a reservation reports about what it reserves is
/// read from the catalog entry the purchase named.
pub mod reservations;

pub use reservations::{
    reservation_not_expired_message, reservation_start, reservation_state, reservation_time,
    NewReservation, ReservationDeleteRefusal, ReservationRecord, ReservationRefusal,
    ReservationStartRefusal, ReservationUpdate, DEFAULT_AUTOMATIC_RENEWAL, MAX_RESERVATIONS,
};

/// The ENGINE VERSION slice: the encoder engine versions available to this
/// account, which `ListVersions` reports, in one module of its own for the same
/// reason as the slices above. Its store is the
/// [`ComAmazonawsMedialiveMedialiveState::channel_engine_versions`] member below,
/// seeded when the store comes up because an engine version is not a resource a
/// caller creates, and read through at every call because which of those versions
/// is still available is a question for the clock.
pub mod engine_versions;

pub use engine_versions::{
    channel_engine_version_label, requested_engine_version_pin, ChannelEngineVersionCatalog,
    ChannelEngineVersionRecord, CHANNEL_ENGINE_VERSION_COUNT, CHANNEL_ENGINE_VERSION_INTERVAL,
    CHANNEL_ENGINE_VERSION_LIFETIME, DEFAULT_ENGINE_VERSION,
};

/// The `ChannelState` enum values this slice uses, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="ChannelState")`:
/// CREATING | CREATE_FAILED | IDLE | STARTING | RUNNING | RECOVERING | STOPPING |
/// DELETING | DELETED | UPDATING | UPDATE_FAILED).
pub mod channel_state {
    pub const CREATING: &str = "CREATING";
    pub const CREATE_FAILED: &str = "CREATE_FAILED";
    pub const IDLE: &str = "IDLE";
    pub const STARTING: &str = "STARTING";
    pub const RUNNING: &str = "RUNNING";
    pub const RECOVERING: &str = "RECOVERING";
    pub const STOPPING: &str = "STOPPING";
    pub const DELETING: &str = "DELETING";
    pub const DELETED: &str = "DELETED";
    pub const UPDATING: &str = "UPDATING";
    pub const UPDATE_FAILED: &str = "UPDATE_FAILED";
}

/// The `SdiSourceState` enum values, spelled exactly as the model declares them
/// (`smithy_query(query="shape", shape="SdiSourceState")`: IDLE | IN_USE |
/// DELETED).
pub mod sdi_source_state {
    pub const IDLE: &str = "IDLE";
    pub const IN_USE: &str = "IN_USE";
    pub const DELETED: &str = "DELETED";
}

/// The `NetworkState` enum values this slice uses, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="NetworkState")`: CREATING |
/// CREATE_FAILED | ACTIVE | DELETING | IDLE | IN_USE | UPDATING | DELETE_FAILED |
/// DELETED).
///
/// Only the states the create, the update and the delete lifecycles reach are
/// named here.
/// `CreateNetwork` is asynchronous — `State` is documented "The current state of
/// the Network. Only MediaLive Anywhere can change the state" — so a network
/// starts `CREATING` and the clock owes it `ACTIVE`, the one state the enum names
/// for a network that exists, has not failed and nothing has deleted. Changing a
/// network's settings is asynchronous for the same reason, so an accepted
/// `UpdateNetwork` puts the network in `UPDATING` and the clock owes it `ACTIVE`
/// again ([`NetworkRecord::begin_update`]). The delete is asynchronous too, so an
/// accepted `DeleteNetwork` puts the network in `DELETING` and the clock owes it
/// the terminal `DELETED` the record then keeps ([`begin_delete_network`]). The
/// two `*_FAILED` states belong to failures this emulator does not inject.
/// `IDLE` and `IN_USE` are the
/// two the model documents nothing about for a network (`State` carries the one
/// sentence above and no member says what puts a network in either), so this
/// emulator never claims one: a network's attachments are reported through its
/// `AssociatedClusterIds`
/// ([`ComAmazonawsMedialiveMedialiveState::clusters_using_network`]) rather than
/// guessed at through its state.
pub mod network_state {
    pub const CREATING: &str = "CREATING";
    pub const ACTIVE: &str = "ACTIVE";
    pub const UPDATING: &str = "UPDATING";
    pub const DELETING: &str = "DELETING";
    pub const DELETED: &str = "DELETED";
}

/// The `ChannelClass` enum values (`smithy_query(query="shape",
/// shape="ChannelClass")`: STANDARD | SINGLE_PIPELINE).
pub mod channel_class {
    pub const STANDARD: &str = "STANDARD";
    pub const SINGLE_PIPELINE: &str = "SINGLE_PIPELINE";
}

/// The pipelines a channel of `channel_class` has, numbered as
/// [`PipelineDetail`]`$PipelineId` numbers them.
///
/// The ONE place the class-to-pipeline rule lives (the `ChannelClass` doc: "A
/// standard channel has two encoding pipelines and a single pipeline channel
/// only has one"): [`ChannelRecord::pipelines`] is this for the class a channel
/// HAS, and a write that would give it another class asks this for the class it
/// WOULD have.
pub fn class_pipelines(channel_class: &str) -> ::std::ops::Range<i32> {
    let count = if channel_class == channel_class::SINGLE_PIPELINE {
        1
    } else {
        2
    };
    0..count
}

/// The pipeline a `PipelineId` / `ChannelPipelineIdToRestart` value names, as
/// [`PipelineDetail`]`$PipelineId` numbers it.
///
/// Both enums declare exactly `PIPELINE_0` and `PIPELINE_1`
/// (`smithy_query(query="shape", shape="PipelineId")`, and the `PipelineIds`
/// member's own doc: "Format PIPELINE_0 or PIPELINE_1"), so they are matched
/// exactly rather than by parsing the suffix: a spelling the enums do not
/// declare names no pipeline. `None` for anything else, which every caller
/// refuses. Whether the pipeline it names is one the CHANNEL has is
/// [`ChannelRecord::has_pipeline`], since that depends on the channel's class.
pub fn pipeline_index(id: &str) -> Option<i32> {
    match id {
        "PIPELINE_0" => Some(0),
        "PIPELINE_1" => Some(1),
        _ => None,
    }
}

/// The `SignalMapStatus` values this slice uses, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="SignalMapStatus")`:
/// CREATE_IN_PROGRESS | CREATE_COMPLETE | CREATE_FAILED | UPDATE_IN_PROGRESS |
/// UPDATE_COMPLETE | UPDATE_REVERTED | UPDATE_FAILED | READY | NOT_READY).
pub mod signal_map_status {
    pub const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
    pub const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
    /// Where `StartUpdateSignalMap` leaves a signal map: the update "is
    /// initiated" and answers 202, so the change runs asynchronously and the
    /// clock settles it into [`UPDATE_COMPLETE`].
    pub const UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
    pub const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";
}

/// The `SignalMapMonitorDeploymentStatus` values this slice uses, spelled
/// exactly as the model declares them (`smithy_query(query="shape",
/// shape="SignalMapMonitorDeploymentStatus")`: NOT_DEPLOYED |
/// DRY_RUN_DEPLOYMENT_COMPLETE | DRY_RUN_DEPLOYMENT_FAILED |
/// DRY_RUN_DEPLOYMENT_IN_PROGRESS | DEPLOYMENT_COMPLETE | DEPLOYMENT_FAILED |
/// DEPLOYMENT_IN_PROGRESS | DELETE_COMPLETE | DELETE_FAILED |
/// DELETE_IN_PROGRESS).
///
/// `NOT_DEPLOYED` is where every signal map starts; `StartMonitorDeployment`
/// moves it into the IN_PROGRESS status of the kind of deployment the request
/// asked for — the dry-run one when `DryRun` is set, the real one otherwise —
/// and the clock settles that into the matching COMPLETE status
/// ([`MonitorDeploymentRecord`]).
///
/// The `DELETE_*` pair belongs to `StartDeleteMonitorDeployment`, which
/// "initiates a deployment to delete the monitor of the specified signal map":
/// that deployment runs in `DELETE_IN_PROGRESS` and the clock settles it into
/// `DELETE_COMPLETE`, on the same terms as the deploying kinds above it.
///
/// The `*_FAILED` statuses are deliberately absent: nothing in this emulator
/// makes a deployment fail, so no code may report one as failed.
pub mod signal_map_monitor_deployment_status {
    pub const NOT_DEPLOYED: &str = "NOT_DEPLOYED";
    pub const DEPLOYMENT_IN_PROGRESS: &str = "DEPLOYMENT_IN_PROGRESS";
    pub const DEPLOYMENT_COMPLETE: &str = "DEPLOYMENT_COMPLETE";
    pub const DRY_RUN_DEPLOYMENT_IN_PROGRESS: &str = "DRY_RUN_DEPLOYMENT_IN_PROGRESS";
    pub const DRY_RUN_DEPLOYMENT_COMPLETE: &str = "DRY_RUN_DEPLOYMENT_COMPLETE";
    pub const DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";
    pub const DELETE_COMPLETE: &str = "DELETE_COMPLETE";
}

/// The `InputSecurityGroupState` enum values, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="InputSecurityGroupState")`:
/// IDLE | IN_USE | UPDATING | DELETED).
pub mod input_security_group_state {
    pub const IDLE: &str = "IDLE";
    pub const IN_USE: &str = "IN_USE";
    pub const UPDATING: &str = "UPDATING";
    pub const DELETED: &str = "DELETED";
}

/// The `InputState` enum values this slice uses, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="InputState")`: CREATING |
/// DETACHED | ATTACHED | DELETING | DELETED).
///
/// `CREATING` is NOT one of them: the real service answers `CreateInput` with an
/// input that is already `DETACHED` (the recorded create carries
/// `State=DETACHED`), so an input is returned ready and owes the clock no
/// transition. `DELETING` and `DELETED` are the deletion half of the lifecycle,
/// which a delete (`DeleteInput`, `BatchDelete`) owns: it starts `DELETING` and
/// the clock settles it into the terminal `DELETED` ([`InputRecord::settle`]),
/// which is what the SDK's `InputDeleted` waiter polls `DescribeInput` for.
pub mod input_state {
    pub const DETACHED: &str = "DETACHED";
    pub const ATTACHED: &str = "ATTACHED";
    pub const DELETING: &str = "DELETING";
    pub const DELETED: &str = "DELETED";
}

/// The `InputClass` enum values (`smithy_query(query="shape",
/// shape="InputClass")`: STANDARD | SINGLE_PIPELINE).
pub mod input_class {
    pub const STANDARD: &str = "STANDARD";
    pub const SINGLE_PIPELINE: &str = "SINGLE_PIPELINE";
}

/// The `InputSourceType` enum values (`smithy_query(query="shape",
/// shape="InputSourceType")`: STATIC | DYNAMIC).
pub mod input_source_type {
    pub const STATIC: &str = "STATIC";
    pub const DYNAMIC: &str = "DYNAMIC";
}

/// The `InputNetworkLocation` value of an input that lives in the AWS Cloud —
/// "AWS, for an input existing in the AWS Cloud"
/// (`smithy_query(query="shape", shape="InputNetworkLocation")`, whose other
/// value is the on-premises one). This is the location whose ingest addresses
/// are this service's own public ones.
pub const AWS_INPUT_NETWORK_LOCATION: &str = "AWS";

/// The `InputNetworkLocation` an input that names none gets: the
/// `InputNetworkLocation` doc says "By default all inputs will continue to be
/// AWS inputs" (`smithy_query(query="shape", shape="InputNetworkLocation")`).
pub const DEFAULT_INPUT_NETWORK_LOCATION: &str = AWS_INPUT_NETWORK_LOCATION;

/// The `InputType` values whose sources can carry a dynamic URL: the
/// `Input$InputSourceType` doc — "Certain pull input sources can be dynamic,
/// meaning that they can have their URL's dynamically changes during input switch
/// actions. Presently, this functionality only works with MP4_FILE and TS_FILE
/// inputs."
const DYNAMIC_CAPABLE_INPUT_TYPES: [&str; 2] = ["MP4_FILE", "TS_FILE"];

/// The variable a source URL carries to make a pull input dynamic, per the
/// MediaLive user guide's "Creating a dynamic input" ("in the URL, include the
/// string `$urlPath$` in place of the part of the path that will change").
const DYNAMIC_URL_VARIABLE: &str = "$urlPath$";

/// One PUSH-type input's ingest protocol: the URL scheme the upstream system
/// pushes with, the port the service opens for it, and whether the
/// destination's `StreamName` is part of the endpoint URL's path.
#[derive(Debug, Clone, Copy)]
struct PushIngestProtocol {
    scheme: &'static str,
    port: u16,
    carries_stream_name: bool,
}

impl PushIngestProtocol {
    /// The endpoint URL this protocol's upstream system pushes to -
    /// `InputDestination$Url`, "the endpoint that the customer stream will be
    /// pushed to": the scheme, the endpoint's address and the port, plus the
    /// destination's stream name as the path for the protocol that names one.
    fn endpoint_url(&self, ip: &str, stream_name: Option<&str>) -> String {
        let mut url = format!("{}://{ip}:{}", self.scheme, self.port);
        if self.carries_stream_name {
            if let Some(stream_name) = stream_name.filter(|name| !name.is_empty()) {
                url.push('/');
                url.push_str(stream_name.trim_start_matches('/'));
            }
        }
        url
    }
}

/// The ingest protocol of every `InputType` the service allocates a push
/// endpoint for, keyed by the type name.
///
/// Source: the `InputType` enum (`smithy_query(query="shape",
/// shape="InputType")`) names the types; the push ports are the ones MediaLive
/// opens for a pushed stream - RTMP's registered 1935, and 5000 for the RTP and
/// UDP push inputs. Only RTMP's URL carries the destination's stream name,
/// per `InputDestinationRequest$StreamName`: "A unique name for the location
/// the RTMP stream is being pushed to."
///
/// Every other `InputType` is deliberately absent: the PULL and file types are
/// not pushed to at all, and for the rest (MEDIACONNECT, MEDIACONNECT_ROUTER,
/// INPUT_DEVICE, AWS_CDI, MULTICAST, SDI, SMPTE_2110_RECEIVER_GROUP and the two
/// SRT types) no documented rule gives the `<scheme>://<ip>:<port>` endpoint a
/// form, so none is invented for them.
const PUSH_INGEST_PROTOCOLS: [(&str, PushIngestProtocol); 3] = [
    (
        "RTMP_PUSH",
        PushIngestProtocol {
            scheme: "rtmp",
            port: 1935,
            carries_stream_name: true,
        },
    ),
    (
        "RTP_PUSH",
        PushIngestProtocol {
            scheme: "rtp",
            port: 5000,
            carries_stream_name: false,
        },
    ),
    (
        "UDP_PUSH",
        PushIngestProtocol {
            scheme: "udp",
            port: 5000,
            carries_stream_name: false,
        },
    ),
];

/// The ingest protocol of `input_type`, or `None` when the service allocates no
/// push endpoint for an input of that type.
fn push_ingest_protocol(input_type: Option<&str>) -> Option<PushIngestProtocol> {
    let input_type = input_type?;
    PUSH_INGEST_PROTOCOLS
        .iter()
        .find(|(name, _)| *name == input_type)
        .map(|(_, protocol)| *protocol)
}

/// The `InputType` values whose ingest IS the `Sources` URL list — the PULL
/// types: `Sources` is "The source URLs for a PULL-type input. Every PULL type
/// input needs exactly two source URLs for redundancy. Only specify sources for
/// PULL type Inputs" (`smithy_query(query="operation", operation="CreateInput")`),
/// so an input of one of these types has nothing to ingest from until it carries
/// at least one source URL.
///
/// The two file types are the same pair [`DYNAMIC_CAPABLE_INPUT_TYPES`] names,
/// because what a dynamic URL varies is exactly such a source. Every other
/// `InputType` ingests through a list of its own (`Destinations` for a pushed-to
/// type, `MediaConnectFlows`, `InputDevices`, `SdiSources`, the multicast,
/// receiver-group and SRT settings), so none of them is in this table.
const PULL_SOURCE_INPUT_TYPES: [&str; 4] = ["URL_PULL", "RTMP_PULL", "MP4_FILE", "TS_FILE"];

/// Whether an input of `input_type` ingests through its `Sources`
/// ([`PULL_SOURCE_INPUT_TYPES`]).
fn pulls_from_sources(input_type: Option<&str>) -> bool {
    input_type.is_some_and(|input_type| PULL_SOURCE_INPUT_TYPES.contains(&input_type))
}

/// Base of the pool this service's emulated public addresses come from — a push
/// input's ingest address and a channel pipeline's egress address alike:
/// `198.18.0.0`, the first address of `198.18.0.0/15`, the RFC 2544
/// benchmarking range. Non-routable, so an emulated endpoint never hands a
/// caller an address that belongs to somebody.
const IP_POOL_BASE: u32 = u32::from_be_bytes([198, 18, 0, 0]);

/// `198.18.0.0/15` holds 2^17 addresses; allocation wraps inside it.
const IP_POOL_LEN: u32 = 1 << 17;

/// How long a resource stays in a transient state before the clock makes the next
/// one due: a channel's `CREATING`, `STARTING`, `STOPPING`, `RECOVERING` and
/// `DELETING`, a signal map's `CREATE_IN_PROGRESS` discovery, and a signal map's
/// monitor deployment, which `StartMonitorDeployment` starts in an IN_PROGRESS
/// status.
///
/// CHOSEN, not documented by AWS: the real service takes minutes, and no doc
/// sentence pins a number, so this is the emulator's own span. It is deliberately
/// non-zero so a caller that acts immediately after `CreateChannel` /
/// `StartChannel` / `CreateSignalMap` / `StartUpdateSignalMap` still observes
/// the transient state and the
/// `ConflictException` it produces, exactly as it would against the real service.
pub const TRANSIENT_STATE_SPAN: Duration = Duration::from_millis(500);

/// The id the first channel of a process gets. AWS channel ids are numeric
/// strings (the issue's example is `1234567`); minting them from a counter at a
/// fixed seven-digit width keeps the `BTreeMap` key order equal to the mint
/// order, which is what makes `ListChannels` pagination deterministic.
const FIRST_CHANNEL_ID: u64 = 1_000_000;

/// The most channels this emulator stores for one account.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateChannel` models
/// `TooManyRequestsException`, which is the refusal once the store is full.
pub const MAX_CHANNELS: usize = 100;

/// The most `RequestId` entries kept for `CreateChannel` idempotency.
///
/// At most one token is recorded per stored channel and a token is dropped with
/// the channel it minted, so this cap is never the binding one in practice; it is
/// enforced anyway, by evicting the oldest token, so the map cannot accumulate
/// caller-chosen strings independently of the channels they name.
pub const MAX_IDEMPOTENCY_TOKENS: usize = MAX_CHANNELS;

/// The id the first signal map of a process gets. AWS signal map ids are short
/// strings (`__stringMin7Max11PatternAws097`, i.e. 7 to 11 characters); minting
/// them from a counter at a fixed seven-digit width satisfies that length and
/// keeps the `BTreeMap` key order equal to the mint order. A range of its own,
/// for the same reason as [`FIRST_INPUT_SECURITY_GROUP_ID`]: a caller that
/// passes a channel's or an input's id where a signal map's belongs gets a
/// `NotFoundException` rather than a coincidental hit.
const FIRST_SIGNAL_MAP_ID: u64 = 4_000_000;

/// The most signal maps this emulator stores for one account.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateSignalMap` models
/// `TooManyRequestsException`, which is the refusal once the store is full.
pub const MAX_SIGNAL_MAPS: usize = 100;

/// The id the first SdiSource of a process gets, and the most SDI sources this
/// emulator stores for one account.
///
/// The id is a numeric string, as MediaLive resource ids are, minted from a
/// counter at a fixed seven-digit width so the `BTreeMap` key order equals the
/// mint order and `ListSdiSources` pagination is deterministic. A range of its
/// own keeps the id spaces apart. The ceiling is not a guess at an AWS quota: it
/// exists so a create loop cannot grow the emulator's heap without bound, and
/// `CreateSdiSource`'s own modelled `TooManyRequestsException` is the refusal
/// once the store is full.
const FIRST_SDI_SOURCE_ID: u64 = 7_000_000;

/// See [`FIRST_SDI_SOURCE_ID`].
pub const MAX_SDI_SOURCES: usize = 100;

/// The id the first Network of a process gets, and the most networks this
/// emulator stores for one account.
///
/// The id is a numeric string, as MediaLive resource ids are, minted from a
/// counter at a fixed seven-digit width so the `BTreeMap` key order equals the
/// mint order and `ListNetworks` pagination is deterministic; `Id` is documented
/// as "the resource-id portion of the ARN", so the ARN is built from it. A range
/// of its own keeps the id spaces apart, for the same reason as
/// [`FIRST_SDI_SOURCE_ID`]. The ceiling is not a guess at an AWS quota: it exists
/// so a create loop cannot grow the emulator's heap without bound, and
/// `CreateNetwork`'s own modelled `TooManyRequestsException` is the refusal once
/// the store is full.
const FIRST_NETWORK_ID: u64 = 8_000_000;

/// See [`FIRST_NETWORK_ID`].
pub const MAX_NETWORKS: usize = 100;

/// The most `IpPools` (or `Routes`) entries this emulator accepts on one
/// network.
///
/// A ceiling, not a guess at AWS's published quota: both lists are caller-chosen
/// and the model targets them at plain lists with no length constraint, so
/// nothing else bounds them, and every entry costs heap on a stored record and a
/// member in every response that reports the network. `CreateNetwork` refuses a
/// longer list with its own modelled `BadRequestException`, because the request
/// is what is out of bounds — the same treatment [`MAX_INPUT_ATTACHMENTS`] gets.
pub const MAX_NETWORK_CIDR_ENTRIES: usize = 32;

/// The id the first input security group of a process gets. AWS input security
/// group ids are numeric strings, the same shape as channel and input ids; a
/// range of its own keeps the id spaces apart, so a caller (or a test) that
/// passes a channel's or an input's id where a group id belongs gets a
/// `NotFoundException` rather than a coincidental hit.
const FIRST_INPUT_SECURITY_GROUP_ID: u64 = 3_000_000;

/// The most input security groups this emulator stores for one account.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateInputSecurityGroup`
/// models `TooManyRequestsException`, which is the refusal once the store is
/// full.
pub const MAX_INPUT_SECURITY_GROUPS: usize = 100;

/// The most `WhitelistRules` entries this emulator accepts on one input security
/// group.
///
/// The same ceiling as [`MAX_NETWORK_CIDR_ENTRIES`] and for the same reason: the
/// list is caller-chosen and the model targets it at a plain list with no length
/// constraint, so nothing else bounds it, and every entry costs heap on a stored
/// record that outlives the request plus a member in every response that reports
/// the group. Both operations that write a whitelist model
/// `BadRequestException`, which is the refusal for a longer list — the request is
/// what is out of bounds.
pub const MAX_WHITELIST_RULES: usize = MAX_NETWORK_CIDR_ENTRIES;

/// The id the first input of a process gets. AWS input ids are numeric strings,
/// so they are minted from a counter at a fixed seven-digit width, which keeps
/// the `BTreeMap` key order equal to the mint order.
const FIRST_INPUT_ID: u64 = 1_000_000;

/// The most `InputAttachments` entries this emulator accepts on one channel.
///
/// A ceiling, not a guess at AWS's published quota: the list is caller-chosen
/// and every read that walks it (an input's `AttachedChannels`, a signal map's
/// discovery) costs work per entry, so it is bounded here for the same reason
/// [`MAX_CHANNELS`] bounds the store. `CreateChannel` refuses a longer list with
/// its own modelled `BadRequestException`, because the request is what is out of
/// bounds.
pub const MAX_INPUT_ATTACHMENTS: usize = 32;

/// The most `Destinations` entries this emulator accepts on one channel.
///
/// A ceiling, not a guess at AWS's published quota, and the same kind of bound
/// as [`MAX_INPUT_ATTACHMENTS`] above: the list is caller-chosen and every read
/// that walks it costs work per entry — a signal map's discovery, and the
/// multiplex slice's feeder index, which `settle` builds out of these
/// destinations on every request that could raise a multiplex alert. Bounding it
/// keeps that per-request work a function of the emulator's own ceilings rather
/// than of one caller's request body. `CreateChannel` refuses a longer list with
/// its own modelled `BadRequestException`, because the request is what is out of
/// bounds. Enforced where a caller's destinations are STORED — `CreateChannel`,
/// `UpdateChannel` and `UpdateChannelClass` — because a handler that stores them
/// without applying this bound would stop the per-request cost the multiplex
/// slice's feeder index pays being bounded by this constant. Each words the
/// refusal with [`too_many_channel_entries_message`].
pub const MAX_CHANNEL_DESTINATIONS: usize = 32;

/// The most inputs this emulator stores for one account.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateInput` models
/// `TooManyRequestsException`, which is the refusal once the store is full.
pub const MAX_INPUTS: usize = 100;

/// The most `RequestId` entries kept for `CreateInput` idempotency - the same
/// bound, for the same reason, as [`MAX_IDEMPOTENCY_TOKENS`] on channels.
pub const MAX_INPUT_IDEMPOTENCY_TOKENS: usize = MAX_INPUTS;

/// The most eventbridge rule template groups this emulator stores for one
/// account.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound.
/// `CreateEventBridgeRuleTemplateGroup` models `TooManyRequestsException`, which
/// is the refusal once the store is full.
pub const MAX_EVENT_BRIDGE_RULE_TEMPLATE_GROUPS: usize = 100;

/// The id the first eventbridge rule template group of a process gets.
///
/// The `Id` member targets `__stringMin7Max11PatternAws097` and documents "AWS
/// provided template groups have ids that start with `aws-`", so a
/// caller-created group's id is seven digits; minting from a counter keeps every
/// minted id seven digits wide and the `BTreeMap` key order equal to the mint
/// order. A range of its own keeps the id spaces of the service's resource types
/// apart, as [`FIRST_SDI_SOURCE_ID`] and [`FIRST_INPUT_SECURITY_GROUP_ID`] do.
const FIRST_EVENT_BRIDGE_RULE_TEMPLATE_GROUP_ID: u64 = 5_000_000;

/// The longest `Name` `CreateEventBridgeRuleTemplateGroup` accepts, from its
/// target shape `__stringMin1Max255PatternS`.
pub const MAX_RESOURCE_NAME_LEN: usize = 255;

/// The longest `Description` a monitor resource accepts, from its target shape
/// `__stringMin0Max1024`.
pub const MAX_RESOURCE_DESCRIPTION_LEN: usize = 1024;

/// The longest `RequestId` (`@idempotencyToken`) a create accepts, from its
/// target shape `__stringMin1Max256PatternS`.
pub const MAX_REQUEST_ID_LEN: usize = 256;

/// Check a string member against the length bounds its target shape names,
/// answering the message that refusal carries: the member, the bound and the
/// length that broke it.
///
/// ONE copy of that length rule and of its wording, so the operations that
/// check the same member of the same resource against the same bound — a
/// create and the update beside it — cannot drift into two readings of one
/// constraint. Length is counted in characters, which is the unit a Smithy
/// `@length` constraint counts in.
///
/// A member the request did not carry is `None` and asks for no change, so it
/// breaks no bound.
pub fn bounded_length(
    member: &str,
    value: Option<&str>,
    min: usize,
    max: usize,
) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    let len = value.chars().count();
    if len >= min && len <= max {
        return Ok(());
    }
    Err(format!(
        "{member} must be between {min} and {max} characters long, but was {len}"
    ))
}

/// Check a numeric member against the range its target shape names, answering
/// the message that refusal carries: the member, the bound and the value that
/// broke it.
///
/// The counterpart of [`bounded_length`] for the integer members, and ONE copy
/// of that range rule and of its wording for the same reason: a create and the
/// update beside it check the same member of the same resource against the same
/// range, so neither can drift into its own reading of it.
///
/// A member the request did not carry is `None` and asks for no change, so it
/// breaks no bound.
pub fn bounded_integer(member: &str, value: Option<i32>, min: i32, max: i32) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    if value >= min && value <= max {
        return Ok(());
    }
    Err(format!(
        "{member} must be between {min} and {max}, but was {value}"
    ))
}

/// Check a member against the closed set of values the model declares for it,
/// answering the message that refusal carries: the member, the catalog and the
/// value that is not in it.
///
/// ONE copy of that closed-set rule and of its wording, for the reason
/// [`bounded_length`] states: an enum member names a set the service can act on,
/// and every operation that writes that member is held to the same set.
///
/// A member the request did not carry is `None` and asks for no change, so it
/// names no value outside the set.
pub fn one_of(member: &str, values: &[&str], value: Option<&str>) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    if values.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "{member} must be one of {}, but was {value}",
        values.join(", ")
    ))
}

/// Check a caller-chosen name that IS the identity of the resource it creates,
/// answering the message that refusal carries: the member and the rule it broke.
///
/// A name of this kind is what a later request uses to address the resource —
/// the key it is stored at, and the path segment the operations that name one
/// resource carry — so an empty value names no resource at all and nothing can
/// be stored under it: the request is refused with the operation's own modelled
/// client error instead of creating something no request can name again.
///
/// ONE copy of that rule and of its wording, for the reason [`bounded_length`]
/// states: every operation that keys a resource by a name the caller supplied is
/// held to it, so none can drift into its own reading of it.
pub fn non_empty_identity(member: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{member} must not be empty"));
    }
    Ok(())
}

/// Check a count against the floor the model's `__integerMin1` sets for it,
/// answering the message that refusal carries: the member and the count that
/// broke it.
///
/// ONE copy of that floor and of its wording, for the reason [`bounded_length`]
/// states: a purchase and the update beside it hold the same counts to the same
/// minimum. A member the request did not carry is not a count at all, so it is
/// not refused.
pub fn at_least_one(member: &str, value: Option<i32>) -> Result<(), String> {
    match value {
        Some(count) if count < 1 => Err(format!("{member} must be at least 1, got {count}")),
        _ => Ok(()),
    }
}

/// The largest page a list operation (`ListChannels`, `ListSdiSources`,
/// `DescribeSchedule`, …) answers with: the upper bound of the `MaxResults`
/// range AWS publishes for this service's list operations (1 to 1000). A
/// `MaxResults` within that range is the page size verbatim, a larger one is
/// clamped here, and a request that names none uses this bound — so a caller
/// that asks for more items than this emulator's stores can hold is never
/// handed a short page plus a continuation token. Finite, so one request cannot
/// ask the service to project an unbounded number of resources in one response.
pub const MAX_LIST_PAGE_SIZE: usize = 1000;

/// The page size one paginated request is served at, from the `MaxResults` it
/// carried — or the message its refusal carries when that member asks for no
/// items at all.
///
/// ONE copy of this page-size rule and of its wording, for the reason
/// [`bounded_length`] states: every paginated operation of this service cuts its
/// page by the same rule, so none of them can drift into its own reading of the
/// floor, of the ceiling or of what an absent member asks for. A `MaxResults`
/// below one asks for nothing and is refused by the floor [`at_least_one`]
/// states for a count; a larger one is the page size verbatim up to
/// [`MAX_LIST_PAGE_SIZE`], which is also what a request naming no `MaxResults`
/// is served at — so no single request can ask the service to project an
/// unbounded number of resources in one response.
///
/// The caller maps the message onto the modelled client error its own operation
/// declares, exactly as the operations wrapping [`bounded_length`] and
/// [`at_least_one`] do.
pub fn list_page_size(max_results: Option<i32>) -> Result<usize, String> {
    at_least_one("maxResults", max_results)?;
    Ok(max_results.map_or(MAX_LIST_PAGE_SIZE, |asked_for| {
        (asked_for as usize).min(MAX_LIST_PAGE_SIZE)
    }))
}

/// The number a value spells when it is written the ONE way this service renders
/// a number it minted — plain decimal digits, no sign and no leading zero — or
/// `None` when it is spelled any other way.
///
/// ONE copy of this rule and for the reason [`bounded_length`] states: every
/// `NextToken` and every numeric id this service hands out is rendered by
/// `to_string()` over the number itself, so the only spellings it can ever have
/// handed out are the ones that render back to the same string. A value whose
/// digits do parse but whose spelling differs — a leading `+`, a padding zero —
/// is a value this service never minted, so it continues no walk and names no
/// resource, and the reader that is handed it refuses it instead of honouring it
/// as the number it resembles. `str::parse` admits both of those spellings, so
/// no reader of a token or of a minted id decodes one with `parse` alone.
pub fn minted_number(value: &str) -> Option<u64> {
    let number: u64 = value.parse().ok()?;
    // The rendering the minting sites write is the only spelling of this number
    // the service hands out, so a value that is not that rendering is not one of
    // them, however it parses.
    (number.to_string() == value).then_some(number)
}

/// Where a paginated request resumes in a sequence its `NextToken` walks by
/// POSITION, and the page that cursor reports out of it.
///
/// ONE copy of this positional-cursor rule and of its wording, for the reason
/// [`bounded_length`] states and beside the page-size rule [`list_page_size`]
/// states: an operation of this service that pages over a sequence it DERIVES —
/// a channel's alert log, a cluster's, a channel's schedule — resumes at a place
/// in that sequence, because an entry of it carries nothing that identifies the
/// entry on its own, so none of them can drift into its own reading of what a
/// token means, of which token is refused or of when a token is handed out at
/// all.
///
/// The caller maps the refusal message onto the modelled client error its own
/// operation declares, exactly as the operations wrapping [`list_page_size`] do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListPosition {
    /// How many entries of the sequence the previous pages already reported.
    already_reported: usize,
    /// The page size [`list_page_size`] cut this request at.
    page_size: usize,
}

impl ListPosition {
    /// The cursor one request walks from, out of the `NextToken` it carried and
    /// the page size it is served at — or the message its refusal carries when
    /// that token is not a position at all.
    ///
    /// `position_of` is the noun phrase, carrying its article, naming what the
    /// positions count as the refusal reads it (`an alert`, `a schedule`).
    ///
    /// A request naming no token asks for the first page. A token that is not a
    /// whole number is NOT read as the first page: it continues nothing this
    /// service handed out, so it is refused — and so is a token that spells a
    /// whole number some other way than [`minted_number`] renders it, because
    /// that spelling is not one this service ever handed out either. A position
    /// PAST the end is not refused, because these sequences shrink on their own:
    /// a token handed out for a page that has since been dropped resumes at the
    /// end of the sequence instead of failing a walk that was valid when it
    /// began.
    pub fn read(
        next_token: Option<&str>,
        page_size: usize,
        position_of: &str,
    ) -> Result<Self, String> {
        let already_reported = match next_token {
            Some(token) => minted_number(token)
                .and_then(|position| usize::try_from(position).ok())
                .ok_or_else(|| format!("nextToken {token} is not {position_of} position"))?,
            None => 0,
        };
        Ok(Self {
            already_reported,
            page_size,
        })
    }

    /// The page of `sequence` this cursor reports — each entry carried into the
    /// shape the response reports it as by `report` — and the token that resumes
    /// after it.
    ///
    /// The token is the position the next page resumes at, handed out ONLY while
    /// an entry is left over after this page, so a response carrying none is the
    /// last page. A page is a snapshot of the sequence it was read from and never
    /// a cursor the service holds open: an entry dropped between two pages shifts
    /// the ones after it.
    pub fn page<Entry, Reported>(
        &self,
        sequence: &[Entry],
        report: impl FnMut(&Entry) -> Reported,
    ) -> (Vec<Reported>, Option<String>) {
        let page: Vec<Reported> = sequence
            .iter()
            .skip(self.already_reported)
            .take(self.page_size)
            .map(report)
            .collect();
        let reported = self.already_reported.saturating_add(page.len());
        let next_token = (reported < sequence.len()).then(|| reported.to_string());
        (page, next_token)
    }
}

/// The BATCH slice: the per-resource outcome machinery and the id-count bound
/// every Batch operation of this service answers by, in one module of its own so
/// the three of them cannot drift into three readings of one rule. What stays in
/// this file is the part the rest of the state needs: the per-operation caps
/// below, which [`batch_results::within_id_cap`] is handed.
pub mod batch_results;

/// The most resource ids ONE `BatchDelete` call may name, counting its
/// `ChannelIds`, `InputIds`, `InputSecurityGroupIds` and `MultiplexIds`
/// together.
///
/// A ceiling, not a guess at AWS's published quota: the model targets those
/// members at plain `__listOf__string` with no length constraint, so nothing
/// else bounds them, and each named id costs a derivation over the stores
/// (`attached_channels`, `inputs_using_security_group`, …) and one entry in the
/// response. Capping the batch caps that product — the work done under the
/// state guard and the size of the two result lists — in one place.
/// `BatchDelete` models `BadRequestException`, which is the refusal for a batch
/// above this bound.
pub const MAX_BATCH_DELETE_IDS: usize = 100;

/// The most resource ids ONE `BatchStop` call may name, counting its
/// `ChannelIds` and `MultiplexIds` together.
///
/// The same ceiling as [`MAX_BATCH_DELETE_IDS`] and for the same reason: the
/// model targets both members at plain `__listOf__string` with no length
/// constraint, and each named id costs a lookup and a projection under the state
/// guard plus one entry in the response. `BatchStop` models
/// `BadRequestException`, which is the refusal for a batch above this bound.
pub const MAX_BATCH_STOP_IDS: usize = MAX_BATCH_DELETE_IDS;

/// The most resource ids ONE `BatchStart` call may name, counting its
/// `ChannelIds` and `MultiplexIds` together.
///
/// The same ceiling as [`MAX_BATCH_STOP_IDS`] and for the same reason: the
/// model targets both members at plain `__listOf__string` with no length
/// constraint, and each named id costs a lookup and a projection under the state
/// guard plus one entry in the response. `BatchStart` models
/// `BadRequestException`, which is the refusal for a batch above this bound.
pub const MAX_BATCH_START_IDS: usize = MAX_BATCH_STOP_IDS;

/// The state a channel's in-flight transition settles into once it comes due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settled {
    /// `CREATING`, `STOPPING` and `UPDATING` settle into `IDLE`.
    Idle,
    /// `STARTING` and `RECOVERING` settle into `RUNNING`.
    Running,
    /// `DELETING` settles into the terminal `DELETED`, which the channel keeps:
    /// the record stays in the account, so `DescribeChannel` goes on reporting
    /// the channel in that state instead of losing it. This is what the SDK's
    /// `ChannelDeleted` waiter polls `DescribeChannel` for.
    Deleted,
}

/// A transition the clock owes: the instant it becomes due and what it settles
/// into. Stored, never slept on.
#[derive(Debug, Clone, Copy)]
struct Transition {
    due: SystemTime,
    to: Settled,
}

/// One created channel, as `CreateChannel` stored it plus its lifecycle state.
///
/// The members mirror the model's `Channel` shape. Members the real service
/// computes from infrastructure it owns (a VPC network interface id, an
/// engine-version expiry) are NOT invented here: they stay absent, because this
/// emulator has no such infrastructure to report. The two members the service
/// SELECTS for every channel — its maintenance window and the public address
/// each pipeline egresses from — are stored, because a channel always has them.
#[derive(Debug, Clone)]
pub struct ChannelRecord {
    pub id: String,
    pub arn: String,
    pub state: String,
    pub channel_class: String,
    pub name: Option<String>,
    pub cdi_input_specification: Option<CdiInputSpecification>,
    pub destinations: Vec<OutputDestination>,
    pub encoder_settings: Option<EncoderSettings>,
    pub input_attachments: Vec<InputAttachment>,
    pub input_specification: Option<InputSpecification>,
    pub log_level: Option<String>,
    pub maintenance: Option<MaintenanceStatus>,
    pub role_arn: Option<String>,
    pub tags: HashMap<String, String>,
    pub channel_security_groups: Vec<String>,
    pub anywhere_settings: Option<AnywhereSettings>,
    pub linked_channel_settings: Option<LinkedChannelSettings>,
    pub inference_settings: Option<InferenceSettings>,
    pub vpc: Option<VpcOutputSettings>,
    pub channel_engine_version: Option<String>,
    /// The public address each of this channel's pipelines egresses from, in
    /// pipeline order — `ChannelEgressEndpoint$SourceIp`, "Public IP of where a
    /// channel's output comes from".
    ///
    /// PRIVATE: the only way in is
    /// [`ComAmazonawsMedialiveMedialiveState::allocate_channel_egress_addresses`],
    /// which owns the pool and never reassigns a pipeline's address, and the
    /// only way out is [`ChannelRecord::egress_endpoints`].
    egress_ips: Vec<String>,
    /// The MediaConnect Router handoff settings a request supplied but the
    /// `Channel` response shape reports no member for — stored rather than
    /// dropped, exactly as [`InputRecord::special_router_settings`] is for an
    /// input, so the member a caller wrote is the member the store holds.
    pub special_router_settings: Option<SpecialRouterSettings>,
    /// The pipelines that are restarting right now, as
    /// [`PipelineDetail`]`$PipelineId` numbers them (0 for `PIPELINE_0`), while
    /// the channel is `RECOVERING`. Empty in every other state: a pipeline is
    /// restarting only between the `RestartChannelPipelines` call that asked for
    /// it and the transition that brings it back up, which is what
    /// [`ChannelRecord::settle`] clears.
    restarting_pipelines: Vec<i32>,
    /// This channel's schedule: the actions `BatchUpdateSchedule` has created in
    /// it and not yet deleted. It lives on the channel because that is what owns
    /// it — `DeleteChannel` takes the schedule with it, and no operation reaches
    /// a schedule without naming its channel.
    pub schedule: ChannelSchedule,
    /// This channel's alert log, which `ListAlerts` reports: it lives on the
    /// channel for the same reason the schedule does. Empty until something
    /// raises an alert on this channel ([`Self::raise_alert`]); see the
    /// [`channel_alerts`] module for why nothing in this emulator does so on its
    /// own. Read for reporting through [`Self::reportable_alerts`], which is
    /// where an alert's pipeline is resolved against this channel's class.
    pub alerts: ChannelAlerts,
    /// The thumbnail frames this channel's pipelines have captured, oldest
    /// first — the [`thumbnails`] slice's store. Nothing in this emulator
    /// captures one, so this is empty unless
    /// [`ComAmazonawsMedialiveMedialiveState::capture_channel_thumbnail`] put
    /// something in it. Private, and written ONLY through
    /// [`ChannelRecord::push_thumbnail_bounded`] just below, which is where the
    /// bound ([`MAX_CHANNEL_THUMBNAILS`]) is enforced; the capture writer owns
    /// the conditions under which a frame can have been captured at all.
    thumbnails: Vec<ThumbnailRecord>,
    /// The transition the clock owes this channel, if any.
    transition: Option<Transition>,
    /// The instant this channel entered the state it is in NOW: the instant of
    /// the call for a state a mutation put it in, and the instant the
    /// transition came DUE for one the clock settled it into — never the
    /// instant of the call that happened to materialise that transition.
    ///
    /// PRIVATE, read through [`Self::state_since`]: whether this channel's
    /// pipelines are up is what the multiplex slice observes an alert condition
    /// about a multiplex program from, and such a condition became true (or
    /// stopped being true) when this channel last changed state, so this is the
    /// instant the alert it raises carries.
    state_since: SystemTime,
}

impl ChannelRecord {
    /// Append `thumbnail` as the newest frame of this channel's capture log,
    /// dropping the OLDEST frames first so the log never holds more than
    /// [`MAX_CHANNEL_THUMBNAILS`] of them.
    ///
    /// The clamp lives here, beside the private store it bounds and BEFORE the
    /// push, because the log is caller-driven — one frame per capture — so an
    /// unbounded log would grow the emulator's heap without end, exactly as
    /// [`MAX_CLUSTER_ALERTS`] bounds the cluster slice's alert log. The field is
    /// private and this is its only writer, so no caller can push past the
    /// bound.
    pub(crate) fn push_thumbnail_bounded(&mut self, thumbnail: ThumbnailRecord) {
        while self.thumbnails.len() >= MAX_CHANNEL_THUMBNAILS {
            self.thumbnails.remove(0);
        }
        self.thumbnails.push(thumbnail);
    }

    /// The number of encoding pipelines this channel's class gives it: two for
    /// `STANDARD`, one for `SINGLE_PIPELINE` (the `ChannelClass` doc: "A standard
    /// channel has two encoding pipelines and a single pipeline channel only has
    /// one").
    pub fn pipeline_count(&self) -> i32 {
        self.pipelines().end
    }

    /// The pipelines this channel's class gives it, numbered as
    /// `PipelineDetail$PipelineId` numbers them (the `ChannelClass` doc: a
    /// `STANDARD` channel has two, a `SINGLE_PIPELINE` channel one).
    ///
    /// Every other reading of the class-to-pipeline rule — whether a request may
    /// name a pipeline, which of them are up, how a schedule action's pause
    /// target is resolved — derives from here, which derives from the one place
    /// that rule lives ([`class_pipelines`]).
    pub fn pipelines(&self) -> ::std::ops::Range<i32> {
        class_pipelines(&self.channel_class)
    }

    /// The pipelines of this channel that are up right now, numbered as
    /// `PipelineDetail$PipelineId` numbers them.
    ///
    /// All of the class's pipelines exactly while the channel is `RUNNING`; while
    /// it is `RECOVERING`, the ones that are not restarting, because a pipeline
    /// `RestartChannelPipelines` took down is not up until the restart completes;
    /// none at all in any other state, where the channel is not serving.
    pub fn healthy_pipelines(&self) -> Vec<i32> {
        let running = self.state == channel_state::RUNNING;
        let recovering = self.state == channel_state::RECOVERING;
        if !running && !recovering {
            return Vec::new();
        }
        self.pipelines()
            .filter(|pipeline| !self.restarting_pipelines.contains(pipeline))
            .collect()
    }

    /// Record an alert the service has observed on this channel, and answer the
    /// `Id` it was given — or `None` when the observation names a pipeline this
    /// channel has not got.
    ///
    /// The one way an alert enters a channel's log, so the `PipelineId` rule
    /// holds for every alert in it: `ChannelAlert$PipelineId` is "The ID of the
    /// pipeline this alert is associated with", and a pipeline this channel's
    /// class does not give it ([`Self::has_pipeline`]) is not one an alert can be
    /// associated with — so there is no such alert to record, and the `None` says
    /// so rather than quietly recording one with the pipeline dropped.
    pub fn raise_alert(
        &mut self,
        alert_type: &str,
        message: Option<&str>,
        pipeline: Option<i32>,
        now: SystemTime,
    ) -> Option<String> {
        if pipeline.is_some_and(|pipeline| !self.has_pipeline(pipeline)) {
            return None;
        }
        Some(
            self.alerts
                .raise(&self.id, alert_type, message, pipeline, now),
        )
    }

    /// Whether `pipeline` is one of the pipelines this channel's class gives it
    /// ([`Self::pipelines`]), so a request may name it.
    pub fn has_pipeline(&self, pipeline: i32) -> bool {
        self.pipelines().contains(&pipeline)
    }

    /// The alerts of this channel's log that a `StateFilter` admits and this
    /// channel has the pipeline for, in the order the service set them — what
    /// `ListAlerts` reports.
    ///
    /// `ChannelAlert$PipelineId` is "The ID of the pipeline this alert is
    /// associated with", and the pipelines a channel has are the ones its class
    /// gives it ([`Self::pipelines`]), so the rule [`Self::raise_alert`] applies
    /// when an alert is recorded is applied again at every READING of the log:
    /// the class a later write stores is what decides which pipelines the
    /// channel has, and an alert associated with one it has not got describes
    /// nothing the channel is running. Whatever state the alert is in, since the
    /// pipeline it names is what the association is.
    ///
    /// The record itself is KEPT, exactly as the address a dropped pipeline
    /// egressed from is ([`Self::egress_endpoints`]) and as the frames it
    /// captured are ([`Self::latest_thumbnail`]), so a channel put back into the
    /// class that gives it that pipeline reports its alerts again.
    pub fn reportable_alerts(&self, filter: Option<&str>) -> Vec<&ChannelAlertRecord> {
        self.alerts
            .in_set_order(filter)
            .into_iter()
            .filter(|alert| {
                alert
                    .pipeline_id
                    .is_none_or(|pipeline| self.has_pipeline(pipeline))
            })
            .collect()
    }

    /// How many of this channel's pipelines are healthy — the
    /// `PipelinesRunningCount` member's own definition ("The number of currently
    /// healthy pipelines"), i.e. how many [`Self::healthy_pipelines`] names.
    pub fn pipelines_running_count(&self) -> i32 {
        i32::try_from(self.healthy_pipelines().len()).unwrap_or(i32::MAX)
    }

    /// `PipelineDetail` is documented as "Runtime details of a pipeline when a
    /// channel is running", so there is one entry per healthy pipeline and none
    /// at all when the channel is not running. Only `PipelineId` is reported: the
    /// other members describe media actually being ingested, which this emulator
    /// does not run and therefore does not claim.
    pub fn pipeline_details(&self) -> Vec<PipelineDetail> {
        self.healthy_pipelines()
            .into_iter()
            .map(|pipeline| PipelineDetail {
                pipeline_id: Some(pipeline.to_string()),
                ..PipelineDetail::default()
            })
            .collect()
    }

    /// The endpoints this channel's outgoing connections initiate from —
    /// `EgressEndpoints`, "The endpoints where outgoing connections initiate
    /// from", one `ChannelEgressEndpoint` per pipeline carrying the public
    /// address allocated to that pipeline.
    ///
    /// One entry per pipeline the channel's class gives it
    /// ([`Self::pipeline_count`]), in pipeline order, so a `STANDARD` channel
    /// reports two and a `SINGLE_PIPELINE` channel one — in every state,
    /// because the address a channel egresses from is provisioned with the
    /// channel and not with a run. A class change that drops a pipeline stops
    /// reporting its address but keeps it recorded, so changing back puts the
    /// same address back on the same pipeline.
    pub fn egress_endpoints(&self) -> Vec<ChannelEgressEndpoint> {
        let pipelines = usize::try_from(self.pipeline_count()).unwrap_or(0);
        self.egress_ips
            .iter()
            .take(pipelines)
            .map(|source_ip| ChannelEgressEndpoint {
                source_ip: Some(source_ip.clone()),
            })
            .collect()
    }

    /// Whether this channel may be started, stopped or deleted right now.
    pub fn is_idle(&self) -> bool {
        self.state == channel_state::IDLE
    }

    /// Whether this channel has stopped: it is `IDLE`, or it failed and never
    /// started, so it is ingesting through no input. The notion `DeleteChannel`
    /// and `UpdateInput`'s conflict check share, so the two cannot disagree
    /// about which states count as settled.
    pub fn has_stopped(&self) -> bool {
        self.is_idle()
            || self.state == channel_state::CREATE_FAILED
            || self.state == channel_state::UPDATE_FAILED
    }

    /// Whether this channel's pipelines are UP on the hardware it is placed on:
    /// a start has been accepted for it and a stop has not finished, so it is
    /// coming up (`STARTING`), up (`RUNNING`), bringing a pipeline back up
    /// (`RECOVERING`) or coming down (`STOPPING`).
    ///
    /// The notion "this channel is running work" — what makes the MediaLive
    /// Anywhere resource it runs on busy rather than idle
    /// ([`ComAmazonawsMedialiveMedialiveState::channels_running_on_node`]). A
    /// channel that is being created, having its configuration applied, waiting
    /// `IDLE` or on its way out of the account runs nothing: `PipelineDetail`
    /// is "Runtime details of a pipeline when a channel is running", and those
    /// are exactly the states [`Self::healthy_pipelines`] reports none for.
    pub fn is_on_air(&self) -> bool {
        matches!(
            self.state.as_str(),
            channel_state::STARTING
                | channel_state::RUNNING
                | channel_state::RECOVERING
                | channel_state::STOPPING
        )
    }

    /// Whether this channel is in a state `DeleteChannel` accepts: a channel is
    /// deletable once it has stopped (`IDLE`) or has failed, never while it is
    /// creating, starting, running, stopping, already deleting or deleted.
    pub fn is_deletable(&self) -> bool {
        self.has_stopped()
    }

    /// Whether this channel has reached the terminal `DELETED` state, which it
    /// then keeps: the record stays in the account, so a reader goes on
    /// reporting the channel in that state instead of losing it.
    pub fn is_deleted(&self) -> bool {
        self.state == channel_state::DELETED
    }

    /// Whether a delete has already been accepted for this channel — it is in
    /// the transient `DELETING` state the clock owes the terminal one, or in
    /// that terminal `DELETED` state itself.
    ///
    /// The notion a MUTATION of the channel resolves against: an id whose
    /// delete the service has accepted names no channel a caller may still
    /// change, exactly as [`InputRecord::is_delete_accepted`] does for an
    /// input.
    pub fn is_delete_accepted(&self) -> bool {
        self.is_deleted() || self.state == channel_state::DELETING
    }

    /// The instant this channel entered the state it is in now — the one thing
    /// anything derived from a channel's state can be dated by.
    pub fn state_since(&self) -> SystemTime {
        self.state_since
    }

    /// Put the channel into `state` as of `now` and owe `to` once
    /// [`TRANSIENT_STATE_SPAN`] has passed.
    fn begin_transition(&mut self, state: &str, to: Settled, now: SystemTime) {
        self.state = state.to_string();
        self.state_since = now;
        self.transition = Some(Transition {
            due: now + TRANSIENT_STATE_SPAN,
            to,
        });
    }

    /// Materialise the transition the clock has already made due, and answer
    /// which one it was — `None` when nothing was owed yet, so a caller can act
    /// on the step that happened on THIS call rather than re-scanning settled
    /// records.
    fn settle(&mut self, now: SystemTime) -> Option<Settled> {
        // The schedule's own clock-owed cleanup: an action whose start time is
        // more than an hour past is no longer in the schedule. Owed whatever the
        // channel's own transition is doing, so it runs first.
        self.schedule.settle(now);
        let transition = self.transition?;
        if now < transition.due {
            return None;
        }
        // A restart that is over leaves no pipeline down, and a channel that has
        // settled — idle, running or deleted — is restarting none.
        self.restarting_pipelines.clear();
        self.state = match transition.to {
            Settled::Idle => channel_state::IDLE,
            Settled::Running => channel_state::RUNNING,
            Settled::Deleted => channel_state::DELETED,
        }
        .to_string();
        // The state was owed at `transition.due` whether or not anybody called,
        // so that — and not this call's clock — is when the channel entered it.
        self.state_since = transition.due;
        self.transition = None;
        Some(transition.to)
    }
}

/// Why a channel MUTATION could not resolve the `ChannelId` it was given, as
/// [`ComAmazonawsMedialiveMedialiveState::mutable_channel`] reports it.
///
/// Carries what a refusal needs and nothing else, so each operation answers in
/// the error shape its own model declares (a 404 for most of them, the 400
/// `UpdateChannel` models instead) without re-reading the store.
#[derive(Debug, Clone)]
pub struct DeadChannel {
    /// The message the refusal carries: `already deleted` for a tombstone the
    /// account still holds, `not found` for an id it never held.
    pub message: String,
    /// The ARN of the record the account still holds under this id, when it
    /// holds one — a tombstone keeps its ARN, and the batch operations report it
    /// beside the per-entry refusal. `None` for an id no channel ever had.
    pub arn: Option<String>,
}

/// One created eventbridge rule template group, as
/// `CreateEventBridgeRuleTemplateGroup` stored it.
///
/// `Description` is `Option` because the member is documented "A resource's
/// optional description": a group created without one has none, rather than an
/// empty one. `created_at` / `modified_at` are the only computed members, and
/// `region` is what scopes the group's name uniqueness and the identifier
/// lookups.
#[derive(Debug, Clone)]
pub struct EventBridgeRuleTemplateGroupRecord {
    pub id: String,
    pub arn: String,
    /// The region the creating request was signed for, which is also the region
    /// [`Self::arn`] names.
    pub region: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
}

/// Which `State` an input security group projection reports — the one member
/// [`ComAmazonawsMedialiveMedialiveState::input_security_group_description`] and
/// [`ComAmazonawsMedialiveMedialiveState::input_security_group_usage_description`]
/// differ by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupStateView {
    /// The terminal `DELETED` once the group has been deleted, and otherwise the
    /// state its users imply: what every READ of the group reports.
    Terminal,
    /// The state the group's users imply, whether or not the group has been
    /// deleted: what the recorded `DeleteInputSecurityGroup` response carries.
    ///
    /// Both recorded rows this view answers, from replaying the
    /// `medialive-input-security-group-lifecycle` fixture — the only evidence
    /// for it, so it is recorded here rather than left to be rediscovered:
    ///
    /// * `#task#6`, the delete of a group nothing uses: reporting the terminal
    ///   `DELETED` diverged at `$.state`, and reporting the users' state (here
    ///   `IDLE`) matches. Computing the description BEFORE marking the group
    ///   deleted would also answer this row.
    /// * `#task#8`, a SECOND delete of that same, already deleted group: it too
    ///   diverged at `$.state` on `DELETED`, and matches on the users' state —
    ///   which a pre-mutation snapshot could NOT produce, since the group was
    ///   already deleted when the call arrived. That row is why the view exists
    ///   rather than an ordering of the write.
    ///
    /// `#task#7` and `#task#9`, the reads between and after those deletes, both
    /// want the terminal `DELETED`, which is [`Self::Terminal`].
    Usage,
}

/// One created input security group, as `CreateInputSecurityGroup` stored it.
///
/// Only the members the request supplies are stored. `State`, `Inputs` and
/// `Channels` are NOT stored: the model documents them as what currently uses
/// the group, so they are derived from the rest of the state on every read
/// ([`ComAmazonawsMedialiveMedialiveState::input_security_group_description`]).
#[derive(Debug, Clone)]
pub struct InputSecurityGroupRecord {
    pub id: String,
    pub arn: String,
    /// The whitelisted IPv4 CIDRs, in the order the request listed them.
    pub whitelist_rules: Vec<String>,
    pub tags: HashMap<String, String>,
    /// Whether this group has been deleted, which is what puts it in the
    /// terminal `DELETED` state. The record stays in the account, so a reader
    /// goes on reporting the group in that state instead of losing it — the
    /// same shape [`SdiSourceRecord::is_deleted`] gives a deleted SdiSource.
    pub deleted: bool,
}

impl InputSecurityGroupRecord {
    /// Whether this group has reached the terminal `DELETED` state.
    pub fn is_deleted(&self) -> bool {
        self.deleted
    }
}

/// Put an input security group into the terminal `DELETED` state. There is no
/// transient step: `InputSecurityGroupState` models none between `IDLE`/`IN_USE`
/// and `DELETED`, so the delete is done with the call. Deleting a group that is
/// already `DELETED` changes nothing.
///
/// A SOFT delete: the record stays addressable through
/// `DescribeInputSecurityGroup` and stays MUTABLE, and the only thing the delete
/// takes away is the group's place in `ListInputSecurityGroups`.
pub fn delete_input_security_group(group: &mut InputSecurityGroupRecord) {
    group.deleted = true;
}

/// Whether `cidr` is an IPv4 CIDR block, which is what
/// `CreateInputSecurityGroup`'s `WhitelistRules` member documents it accepts
/// ("List of IPv4 CIDR addresses to whitelist",
/// `smithy_query(query="doc", shape="InputWhitelistRuleCidr", member="Cidr")`:
/// "The IPv4 CIDR to whitelist").
///
/// Dotted-quad address, `/`, then a prefix length of 0..=32 — the published
/// bounds of an IPv4 prefix. The model declares no `@pattern` for it, so this is
/// the service-side rule the boundary validator cannot express.
pub fn is_ipv4_cidr(cidr: &str) -> bool {
    let Some((address, prefix)) = cidr.split_once('/') else {
        return false;
    };
    if prefix.is_empty() || !prefix.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let Ok(prefix_length) = prefix.parse::<u32>() else {
        return false;
    };
    prefix_length <= 32 && address.parse::<::std::net::Ipv4Addr>().is_ok()
}

/// The message a request whose whitelist rules are not all valid IPv4 CIDRs is
/// refused with — the fixed sentence of that rejection class, which states the
/// rule the whole list must meet and interpolates nothing: the service refuses
/// the request as a whole rather than offering a per-member error contract, so
/// neither the index of the offending rule nor the value it carried is part of
/// the message.
pub const INVALID_WHITELIST_CIDR_MESSAGE: &str = "Every whitelist needs to have a valid IPv4 CIDR.";

/// The message a request carrying more whitelist rules than this emulator keeps
/// on one group is refused with: the ceiling and the member as the wire spells
/// it, worded as the network lists' own ceiling is
/// ([`too_many_network_entries_message`]).
pub fn too_many_whitelist_rules_message() -> String {
    format!("An input security group takes at most {MAX_WHITELIST_RULES} whitelistRules entries.")
}

/// The `WhitelistRules` a request asks for, validated into the CIDR strings the
/// store keeps, or the message a list carrying anything that is not a valid IPv4
/// CIDR is refused with.
///
/// ONE copy for every operation that writes a group's whitelist
/// (`CreateInputSecurityGroup` and `UpdateInputSecurityGroup`), which document
/// the member identically ("List of IPv4 CIDR addresses to whitelist"), so the
/// rule the two enforce cannot drift apart. The whole list is checked before the
/// caller writes anything, so a refused request leaves the store untouched
/// rather than applying a prefix of its rules.
///
/// A rule that names no CIDR at all fails the same rule and so carries the same
/// message: every whitelist needs a valid IPv4 CIDR, and a missing one is not a
/// valid one.
///
/// The list's LENGTH is bounded here too ([`MAX_WHITELIST_RULES`]), for the same
/// reason the network lists are: every accepted entry is kept on a record that
/// outlives the request, so a longer list is refused before any of it is stored
/// rather than growing the store by whatever one request chose.
pub fn whitelist_rules_from_request(
    requested: Vec<InputWhitelistRuleCidr>,
) -> Result<Vec<String>, String> {
    // Not sized from the request: the capacity a caller asks for is not an
    // allocation this emulator makes before it has accepted the entries.
    let mut cidrs: Vec<String> = Vec::new();
    for rule in requested {
        if cidrs.len() >= MAX_WHITELIST_RULES {
            return Err(too_many_whitelist_rules_message());
        }
        let cidr = rule.cidr.filter(|cidr| is_ipv4_cidr(cidr));
        let Some(cidr) = cidr else {
            return Err(INVALID_WHITELIST_CIDR_MESSAGE.to_string());
        };
        cidrs.push(cidr);
    }
    Ok(cidrs)
}

/// A bounded map from a caller-chosen `@idempotencyToken` to the id of the
/// resource that token created, so a retried create returns the resource the
/// first attempt made instead of a second one.
///
/// One implementation for every resource kind that models an idempotency token
/// (channels, inputs, SDI sources, signal maps, template groups), so the cap and
/// the eviction order cannot drift between them, and a token used on two
/// different creates cannot make one of them answer with the other's resource.
/// The fields are PRIVATE: the only way in is [`Self::record`], which enforces
/// the cap by evicting the oldest token.
#[derive(Debug, Default)]
pub struct IdempotencyTokens {
    /// token -> the id of the resource it created.
    tokens: HashMap<String, String>,
    /// The order [`Self::tokens`] was written in, so the cap evicts the oldest
    /// token rather than an arbitrary one.
    order: VecDeque<String>,
}

impl IdempotencyTokens {
    /// Record `token` as having created `resource_id`, evicting the oldest token
    /// first if that would push the map past `cap`.
    fn record(&mut self, token: String, resource_id: &str, cap: usize) {
        while self.tokens.len() >= cap {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.tokens.remove(&oldest);
                }
                // The order queue and the map are written together, so this is
                // unreachable; clearing rather than looping forever keeps the cap
                // an invariant even if they ever drift.
                None => {
                    self.tokens.clear();
                    break;
                }
            }
        }
        if self
            .tokens
            .insert(token.clone(), resource_id.to_string())
            .is_none()
        {
            self.order.push_back(token);
        }
    }

    /// The id `token` created, whether or not that resource still exists.
    fn created(&self, token: &str) -> Option<&str> {
        self.tokens.get(token).map(String::as_str)
    }

    /// Drop every token that created `resource_id`, so a token never outlives
    /// the resource it minted.
    fn forget(&mut self, resource_id: &str) {
        self.tokens.retain(|_, created| created != resource_id);
        let live = &self.tokens;
        self.order.retain(|token| live.contains_key(token));
    }
}

/// Project a [`DescribeNetworkSummary`] onto one of the structurally identical
/// per-operation network output shapes (`CreateNetworkOutput`,
/// `DescribeNetworkOutput`, `DeleteNetworkOutput`, `UpdateNetworkOutput`, which
/// all carry the same seven members).
///
/// One macro rather than a hand-written copy of the same seven-member move per
/// operation, so the operations that report a network cannot drift from each
/// other. `DescribeNetworkSummary` is the shape a LISTING reports, which makes
/// the listed summary and the described network one projection rather than two.
macro_rules! network_description_output {
    ($out:ident, $network:expr) => {{
        let network: $crate::ops::types::DescribeNetworkSummary = $network;
        $out {
            arn: network.arn,
            associated_cluster_ids: network.associated_cluster_ids,
            id: network.id,
            ip_pools: network.ip_pools,
            name: network.name,
            routes: network.routes,
            state: network.state,
        }
    }};
}

pub(crate) use network_description_output;

/// Why a monitor template GROUP's delete removed nothing. Each variant is one
/// modelled refusal, and both of them leave the store exactly as it was.
///
/// ONE enum for the two group resources
/// ([`ComAmazonawsMedialiveMedialiveState::delete_alarm_template_group`] and
/// [`ComAmazonawsMedialiveMedialiveState::delete_event_bridge_rule_template_group`]),
/// whose deletes document the SAME precondition sentence, so the two cannot
/// disagree about which refusal an unmet precondition is. A delete that documents
/// no precondition (the eventbridge rule TEMPLATE's) does not use this type: it
/// answers the record it removed or nothing at all.
#[derive(Debug)]
pub enum TemplateGroupDeleteRefusal {
    /// No group of that type in the request's region answers to the identifier,
    /// by either form `Identifier` documents (its id or its current name).
    NotFound,
    /// Something still holds the group, and its delete documents that the caller
    /// "must detach this group from all signal maps and ensure its existing
    /// templates are moved to another group or deleted" first. Each list carries
    /// the ids that still hold it, and is empty when that half of the
    /// precondition is met (or, for a group whose templates have no store yet,
    /// when there is nothing of that kind to check).
    StillHeld {
        signal_map_ids: Vec<String>,
        template_ids: Vec<String>,
    },
    /// The identifier names a group the SERVICE publishes
    /// ([`managed_monitor_templates`]) rather than one the account created, so
    /// the account cannot delete it however its own state stands.
    Published,
}

/// The message a group delete refused because the group still holds templates
/// carries — the fixed sentence of that rejection class, which states the
/// precondition both group deletes document ("ensure its existing templates are
/// moved to another group or deleted") and interpolates nothing: the group's
/// identifier is in the request's own URI and the ids of the templates it still
/// holds are state the caller did not supply.
pub const TEMPLATE_GROUP_NOT_EMPTY_MESSAGE: &str = "Template group must be empty before deletion.";

/// The message a group delete refused because a signal map still names the
/// group carries: the other half of the precondition both group deletes
/// document ("You must detach this group from all signal maps"), in the same
/// fixed form as [`TEMPLATE_GROUP_NOT_EMPTY_MESSAGE`] and interpolating nothing
/// for the same reason.
pub const TEMPLATE_GROUP_STILL_ATTACHED_MESSAGE: &str =
    "Template group must be detached from all signal maps before deletion.";

/// The message a group delete's conflict carries: the unmet half of the
/// precondition both group deletes document ("You must detach this group from
/// all signal maps and ensure its existing templates are moved to another group
/// or deleted"), as the fixed sentence of that rejection class.
///
/// ONE builder, so the two group deletes cannot word the same refusal two ways.
/// The halves are reported in the order the documentation states them: a group a
/// signal map still names must be detached first, and only then does its
/// remaining content matter.
pub fn still_held_message(signal_map_ids: &[String], template_ids: &[String]) -> String {
    if signal_map_ids.is_empty() && !template_ids.is_empty() {
        TEMPLATE_GROUP_NOT_EMPTY_MESSAGE.to_string()
    } else {
        TEMPLATE_GROUP_STILL_ATTACHED_MESSAGE.to_string()
    }
}

/// One REGION's resources for com-amazonaws-medialive-medialive. A handler
/// reaches the partition of the region its own request was signed for through
/// [`service_state`], and no other
/// ([`ComAmazonawsMedialiveMedialiveStore`] is what holds them all).
#[derive(Debug, Default)]
pub struct ComAmazonawsMedialiveMedialiveState {
    /// Every channel that exists, keyed by its minted `Id`. A `BTreeMap` so
    /// `ListChannels` walks it in id order on every call and its `NextToken`
    /// means the same thing on the next one.
    pub channels: BTreeMap<String, ChannelRecord>,
    /// Every input security group that exists, keyed by its minted `Id`. A
    /// `BTreeMap` for the same reason as [`Self::channels`]: a reader walks it in
    /// id order on every call.
    pub input_security_groups: BTreeMap<String, InputSecurityGroupRecord>,
    /// `CreateChannel`'s `RequestId` (`@idempotencyToken`) -> the channel it
    /// created, so a retry returns that channel instead of a second one.
    ///
    /// PRIVATE, so the cap and the eviction order are invariants the type
    /// enforces: it is written only by [`Self::record_idempotency_token`] and
    /// read only through [`Self::channel_for_token`].
    channel_tokens: IdempotencyTokens,
    /// Every input that exists, keyed by its minted `Id`, in id order for the
    /// same reason as [`Self::channels`].
    pub inputs: BTreeMap<String, InputRecord>,
    /// `CreateInput`'s `RequestId` (`@idempotencyToken`) -> the input it created.
    input_tokens: IdempotencyTokens,
    /// The id mints every region partition of one store SHARES, so no two
    /// regions can hand out the same id and no address this service reports in
    /// one region is reported in another. Set by
    /// [`ComAmazonawsMedialiveMedialiveStore::region`] when it creates the
    /// partition.
    ///
    /// PRIVATE: every id comes out of a `mint_*` method of this store, which is
    /// where the kind of id and its first value live.
    mints: ::std::sync::Arc<IdMints>,
    /// Every cloudwatch alarm template group that exists — the ones this account
    /// created, keyed by their minted `Id`, and the ones the SERVICE publishes,
    /// keyed by the `aws-` prefixed id the [`managed_monitor_templates`] catalog
    /// publishes them under and seeded when the partition came up. A `BTreeMap`
    /// so a later `ListCloudWatchAlarmTemplateGroups` walks it in id order on
    /// every call.
    ///
    /// PRIVATE, so the name-uniqueness rule, the cap and the mint stay invariants
    /// of [`Self::insert_alarm_template_group`] rather than of each handler.
    alarm_template_groups: BTreeMap<String, CloudWatchAlarmTemplateGroupRecord>,
    /// `CreateCloudWatchAlarmTemplateGroup`'s `RequestId` (`@idempotencyToken`),
    /// keyed per region by [`template_group_token_key`] -> the group it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    alarm_template_group_tokens: IdempotencyTokens,
    /// Every cloudwatch alarm template that exists — this account's own, keyed by
    /// their minted `Id`, and the ones the SERVICE publishes inside its own
    /// groups, seeded when the partition came up, exactly as
    /// [`Self::alarm_template_groups`] holds both provenances. A `BTreeMap` so a
    /// later `ListCloudWatchAlarmTemplates` walks it in id order on every call.
    ///
    /// PRIVATE, so the group resolution, the name-uniqueness rule, the cap and
    /// the mint stay invariants of [`Self::insert_alarm_template`] rather than of
    /// each handler.
    alarm_templates: BTreeMap<String, CloudWatchAlarmTemplateRecord>,
    /// `CreateCloudWatchAlarmTemplate`'s `RequestId` (`@idempotencyToken`), keyed
    /// per region by [`template_group_token_key`] -> the template it created.
    /// PRIVATE for the same reason as [`Self::channel_tokens`].
    alarm_template_tokens: IdempotencyTokens,
    /// Every SdiSource that exists, keyed by its minted `Id`, in a `BTreeMap`
    /// for the same reason as [`Self::channels`].
    pub sdi_sources: BTreeMap<String, SdiSourceRecord>,
    /// `CreateSdiSource`'s `RequestId` (`@idempotencyToken`) -> the SdiSource it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    sdi_source_tokens: IdempotencyTokens,
    /// Every Network that exists, keyed by its minted `Id`, in a `BTreeMap` for
    /// the same reason as [`Self::channels`]: `ListNetworks` walks it in id
    /// order on every call and its `NextToken` means the same thing on the next
    /// one.
    pub networks: BTreeMap<String, NetworkRecord>,
    /// `CreateNetwork`'s `RequestId` (`@idempotencyToken`) -> the network it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    network_tokens: IdempotencyTokens,
    /// Every eventbridge rule template group that exists, keyed by its minted
    /// `Id`. A `BTreeMap` so a walk over the groups is in id order on every
    /// call.
    pub event_bridge_rule_template_groups: BTreeMap<String, EventBridgeRuleTemplateGroupRecord>,
    /// `CreateEventBridgeRuleTemplateGroup`'s `RequestId` (`@idempotencyToken`)
    /// -> the group it created, so a retry returns that group instead of a
    /// second one. PRIVATE for the same reason as [`Self::channel_tokens`].
    event_bridge_rule_template_group_tokens: IdempotencyTokens,
    /// Every eventbridge rule template that exists, keyed by its minted `Id`. A
    /// `BTreeMap` so a later `ListEventBridgeRuleTemplates` walks it in id order
    /// on every call.
    ///
    /// PRIVATE, so the group resolution, the name-uniqueness rule, the cap and
    /// the mint stay invariants of
    /// [`Self::insert_event_bridge_rule_template`] rather than of each handler.
    event_bridge_rule_templates: BTreeMap<String, EventBridgeRuleTemplateRecord>,
    /// `CreateEventBridgeRuleTemplate`'s `RequestId` (`@idempotencyToken`), keyed
    /// per region by [`template_group_token_key`] -> the template it created.
    /// PRIVATE for the same reason as [`Self::channel_tokens`].
    event_bridge_rule_template_tokens: IdempotencyTokens,
    /// Every signal map that exists, keyed by its minted `Id`, in mint order for
    /// the same reason as [`Self::channels`].
    pub signal_maps: BTreeMap<String, SignalMapRecord>,
    /// `CreateSignalMap`'s `RequestId` (`@idempotencyToken`) -> the signal map it
    /// created.
    signal_map_tokens: IdempotencyTokens,
    /// Every multiplex that exists, keyed by its minted `Id`, in id order for
    /// the same reason as [`Self::channels`].
    pub multiplexes: BTreeMap<String, MultiplexRecord>,
    /// `CreateMultiplex`'s `RequestId` (`@idempotencyToken`) -> the multiplex it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    multiplex_tokens: IdempotencyTokens,
    /// `CreateMultiplexProgram`'s `RequestId` (`@idempotencyToken`) -> the NAME
    /// of the program it created, keyed per multiplex by the multiplex slice's
    /// `multiplex_program_token_key` because a program is created against one
    /// multiplex's endpoint and is named by that multiplex together with its own
    /// name rather than by an id. PRIVATE for the same reason as
    /// [`Self::channel_tokens`].
    multiplex_program_tokens: IdempotencyTokens,
    /// Every input device this account has claimed, keyed by the device id the
    /// claim named. A `BTreeMap` for the same reason as [`Self::channels`]: a
    /// reader walks it in id order on every call. There is no mint counter
    /// beside it, because a device id is the unit's own, not one this service
    /// assigns.
    pub input_devices: BTreeMap<String, InputDeviceRecord>,
    /// Every cluster that exists, keyed by its minted `Id`, in id order for the
    /// same reason as [`Self::channels`]: a reader walks it in id order on every
    /// call. The alerts raised against a cluster live on its own record.
    pub clusters: BTreeMap<String, ClusterRecord>,
    /// `CreateCluster`'s `RequestId` (`@idempotencyToken`) -> the cluster it
    /// created. PRIVATE for the same reason as [`Self::channel_tokens`].
    cluster_tokens: IdempotencyTokens,
    /// Every channel placement group that exists, keyed by its minted `Id`. A
    /// `BTreeMap` so a later `ListChannelPlacementGroups` walks it in id order
    /// on every call.
    pub channel_placement_groups: BTreeMap<String, ChannelPlacementGroupRecord>,
    /// `CreateChannelPlacementGroup`'s `RequestId` (`@idempotencyToken`), keyed
    /// per cluster by the slice's `channel_placement_group_token_key` -> the
    /// group it created. PRIVATE for the same reason as [`Self::channel_tokens`].
    channel_placement_group_tokens: IdempotencyTokens,
    /// Every node that exists, in every cluster, keyed by its minted `Id`, in id
    /// order for the same reason as [`Self::channels`]. The cluster a node
    /// belongs to is on the record, and every node lookup matches it
    /// ([`Self::node`]), because a node's id is unique in its cluster.
    pub nodes: BTreeMap<String, NodeRecord>,
    /// `CreateNode`'s `RequestId` (`@idempotencyToken`) -> the node it created,
    /// keyed by the node slice's `node_token_key` because a node is created
    /// against one cluster's endpoint. PRIVATE for the same reason as
    /// [`Self::channel_tokens`].
    node_tokens: IdempotencyTokens,
    /// Every node registration script this account has been issued, keyed by the
    /// id the registration script slice minted for it, in id order — which is
    /// also the order they were issued in, so the ring that bounds the map drops
    /// the oldest.
    ///
    /// PRIVATE, so the ring's bound, the per-cluster lookup and the rendering
    /// stay invariants of the [`node_registration_scripts`] slice's own methods
    /// rather than of each handler. The operation reports a script's TEXT and no
    /// id, so nothing outside that slice resolves a record.
    node_registration_scripts: BTreeMap<String, NodeRegistrationScriptRecord>,
    /// `CreateNodeRegistrationScript`'s `RequestId` (`@idempotencyToken`) -> the
    /// script it issued, keyed per cluster by that slice's
    /// `node_registration_script_token_key` because a script is issued against
    /// one cluster's endpoint. PRIVATE for the same reason as
    /// [`Self::channel_tokens`].
    node_registration_script_tokens: IdempotencyTokens,
    /// Every reservation the account has purchased, keyed by its minted `Id`, in
    /// id order for the same reason as [`Self::channels`]: a reader walks it in
    /// id order on every call.
    pub reservations: BTreeMap<String, ReservationRecord>,
    /// `PurchaseOffering`'s `RequestId` (`@idempotencyToken`) -> the reservation
    /// it purchased. PRIVATE for the same reason as [`Self::channel_tokens`].
    reservation_tokens: IdempotencyTokens,
    /// The account's encoder engine version catalogue, as `ListVersions` reports
    /// the available ones out of it.
    ///
    /// There is no mint counter and no token map beside it: an engine version is
    /// not a resource a caller creates, it is one the service publishes, so
    /// nothing on a request path writes this catalogue — and an account the
    /// service has published no version to carries the EMPTY one the
    /// [`engine_versions`] slice's `Default` gives, never a set derived from the
    /// local clock.
    pub channel_engine_versions: ChannelEngineVersionCatalog,
}

impl ComAmazonawsMedialiveMedialiveState {
    /// An empty partition of one region's resources, minting them out of
    /// `mints` — the store's own set, which every partition shares, so no two
    /// regions hand out the same id. The [`regions`] slice is what calls this;
    /// `Default` (which gives a set of its own) is for a standalone store.
    ///
    /// Empty of the ACCOUNT's resources, that is. The monitor template resources
    /// the SERVICE publishes exist in every account and every region without
    /// anybody creating them, so the partition comes up carrying them (the
    /// [`managed_monitor_templates`] catalog), which is what makes a listing
    /// report them on an account that has created nothing. They are seeded here,
    /// once per partition, rather than merged in by each reader, so every reader
    /// — the listings, the single gets, the `Scope` filter and a group's derived
    /// `TemplateCount` — answers from the one store.
    fn with_mints(region: &str, mints: ::std::sync::Arc<IdMints>) -> Self {
        let mut state = Self {
            mints,
            ..Self::default()
        };
        for group in published_groups(region) {
            state.alarm_template_groups.insert(group.id.clone(), group);
        }
        for template in published_templates(region) {
            state.alarm_templates.insert(template.id.clone(), template);
        }
        state
    }

    /// Materialise every transition the clock had already made due as of `now`,
    /// then answer from the result.
    ///
    /// Call this at the top of every operation, reporting ones included: it only
    /// applies transitions that were owed at that instant whether or not anybody
    /// called, so it never changes what a later caller would otherwise have seen.
    pub fn settle(&mut self, now: SystemTime) {
        // Only a record that settles into `DELETED` on THIS call is collected,
        // so the work here does not grow with the number of records that have
        // already reached that terminal state.
        let mut just_deleted: Vec<String> = Vec::new();
        for (id, channel) in &mut self.channels {
            if channel.settle(now) == Some(Settled::Deleted) {
                just_deleted.push(id.clone());
            }
        }
        // A token never outlives the channel it minted: once that channel has
        // reached the terminal `DELETED` state its idempotency window is over, so
        // a retried create mints a new channel rather than answering with a
        // deleted one.
        for id in just_deleted {
            self.channel_tokens.forget(&id);
        }
        // Collected and forgotten on the same terms as the channel tokens
        // above: only an input that settles into `DELETED` on THIS call is
        // collected, and a token never outlives the input it minted, so a
        // retried create mints a new input rather than answering with a
        // deleted one.
        let mut deleted_inputs: Vec<String> = Vec::new();
        for (id, input) in &mut self.inputs {
            if input.settle(now) {
                deleted_inputs.push(id.clone());
            }
        }
        for id in deleted_inputs {
            self.input_tokens.forget(&id);
        }
        for signal_map in self.signal_maps.values_mut() {
            signal_map.settle(now);
        }
        // Collected and forgotten on the same terms as the channel tokens
        // above: only a network that settles into `DELETED` on THIS call is
        // collected, and a token never outlives the network it minted, so a
        // retried create mints a new network rather than answering with a
        // deleted one.
        let mut deleted_networks: Vec<String> = Vec::new();
        for (id, network) in &mut self.networks {
            if network.settle(now) {
                deleted_networks.push(id.clone());
            }
        }
        for id in deleted_networks {
            self.network_tokens.forget(&id);
        }
        self.settle_multiplexes(now);
        self.settle_input_devices(now);
        self.settle_clusters(now);
        self.settle_channel_placement_groups(now);
        self.settle_nodes(now);
    }

    /// Whether the store is full, so `CreateChannel` must refuse rather than mint
    /// another channel.
    ///
    /// A `DELETED` channel still counts: its record stays in the account, so
    /// counting it is what keeps the store — and the emulator's heap — bounded
    /// by [`MAX_CHANNELS`] however many creates and deletes a caller runs.
    pub fn is_full(&self) -> bool {
        self.channels.len() >= MAX_CHANNELS
    }

    /// Record `token` as having created `channel_id`, evicting the oldest token
    /// first if that would push the map past [`MAX_IDEMPOTENCY_TOKENS`].
    pub fn record_idempotency_token(&mut self, token: String, channel_id: &str) {
        self.channel_tokens
            .record(token, channel_id, MAX_IDEMPOTENCY_TOKENS);
    }

    /// The next channel id, as AWS shapes them: a numeric string.
    pub fn mint_channel_id(&mut self) -> String {
        (FIRST_CHANNEL_ID + self.mints.next(|mints| &mut mints.channel)).to_string()
    }

    /// The next input security group id, as AWS shapes them: a numeric string.
    pub fn mint_input_security_group_id(&mut self) -> String {
        (FIRST_INPUT_SECURITY_GROUP_ID
            + self.mints.next(|mints| &mut mints.input_security_group))
            .to_string()
    }

    /// Whether the input security group store is full, so
    /// `CreateInputSecurityGroup` must refuse rather than mint another group.
    pub fn input_security_groups_are_full(&self) -> bool {
        self.input_security_groups.len() >= MAX_INPUT_SECURITY_GROUPS
    }

    /// The ids of the channels currently using `group_id` as one of their
    /// channel security groups — `InputSecurityGroup$Channels` ("The list of
    /// channels currently using this Input Security Group as their channel
    /// security group"). Derived from the channel store, so it tracks every
    /// later create and delete rather than being recorded at create time.
    ///
    /// A channel whose delete this service has ACCEPTED is being torn down and
    /// uses nothing any more, so it is not reported — the member says
    /// *currently*. That is the one cut point this module states for every
    /// derived "currently using" list, and the one the input side below applies.
    pub fn channels_using_security_group(&self, group_id: &str) -> Vec<String> {
        self.channels
            .values()
            .filter(|channel| !channel.is_delete_accepted())
            .filter(|channel| {
                channel
                    .channel_security_groups
                    .iter()
                    .any(|attached| attached == group_id)
            })
            .map(|channel| channel.id.clone())
            .collect()
    }

    /// The ids of the inputs currently using `group_id` as one of their input
    /// security groups — `InputSecurityGroup$Inputs` ("The list of inputs
    /// currently using this Input Security Group"). Derived from the input store
    /// for the same reason as [`Self::channels_using_security_group`], and it
    /// cuts at the same point for the same reason too.
    pub fn inputs_using_security_group(&self, group_id: &str) -> Vec<String> {
        self.inputs
            .values()
            // The SAME cut point the channel side applies, which is the one this
            // module documents for every derived "currently using" list:
            // accepting an input's delete releases the group, so the input stops
            // being one of its current users at that instant — the transient
            // `DELETING` step included — and the group's `State` falls back to
            // IDLE with it.
            .filter(|input| !input.is_delete_accepted())
            .filter(|input| {
                input
                    .security_groups
                    .iter()
                    .any(|attached| attached == group_id)
            })
            .map(|input| input.id.clone())
            .collect()
    }

    /// The full `InputSecurityGroup` description of one stored group, or `None`
    /// when no group has that id.
    ///
    /// `State` is derived, not stored: a deleted group is in the terminal
    /// `DELETED` state it keeps, a group some resource in state references is
    /// `IN_USE`, and one nothing references is `IDLE` ("The current state of the
    /// Input Security Group").
    pub fn input_security_group_description(&self, id: &str) -> Option<InputSecurityGroup> {
        self.described_input_security_group(id, GroupStateView::Terminal)
    }

    /// The same description with `State` derived from the group's users ALONE —
    /// `IN_USE` while some resource references it and `IDLE` while none does —
    /// ignoring whether the group has been deleted.
    ///
    /// This is what `DeleteInputSecurityGroup` answers with: the recorded delete
    /// response reports that usage state both for the group it has just deleted
    /// and for one an earlier delete already deleted, while the terminal
    /// `DELETED` is what every `DescribeInputSecurityGroup` after the delete
    /// reports.
    pub(crate) fn input_security_group_usage_description(
        &self,
        id: &str,
    ) -> Option<InputSecurityGroup> {
        self.described_input_security_group(id, GroupStateView::Usage)
    }

    /// One projection behind both readers above, so the two cannot drift in any
    /// member but the `State` that `view` selects.
    fn described_input_security_group(
        &self,
        id: &str,
        view: GroupStateView,
    ) -> Option<InputSecurityGroup> {
        let group = self.input_security_groups.get(id)?;
        let inputs = self.inputs_using_security_group(id);
        let channels = self.channels_using_security_group(id);
        let state = match view {
            GroupStateView::Terminal => {
                input_security_group_state_for(group, &inputs, &channels)
            }
            GroupStateView::Usage => input_security_group_usage_state(&inputs, &channels),
        };
        Some(InputSecurityGroup {
            arn: Some(group.arn.clone()),
            id: Some(group.id.clone()),
            inputs: Some(inputs),
            state: Some(state.to_string()),
            tags: Some(group.tags.clone()),
            whitelist_rules: Some(
                group
                    .whitelist_rules
                    .iter()
                    .map(|cidr| InputWhitelistRule {
                        cidr: Some(cidr.clone()),
                    })
                    .collect(),
            ),
            channels: Some(channels),
        })
    }

    /// The channel a stored `RequestId` created, if that channel still exists.
    pub fn channel_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.channel_tokens.created(request_id)?;
        self.channels.get(id).map(|channel| channel.id.as_str())
    }

    /// Whether the group store is full, so
    /// `CreateEventBridgeRuleTemplateGroup` must refuse rather than mint
    /// another group.
    pub fn event_bridge_rule_template_groups_full(&self) -> bool {
        self.event_bridge_rule_template_groups.len() >= MAX_EVENT_BRIDGE_RULE_TEMPLATE_GROUPS
    }

    /// The next eventbridge rule template group id: seven digits, so it matches
    /// the `Id` member's `__stringMin7Max11PatternAws097` target without the
    /// `aws-` prefix AWS reserves for the groups it provides itself.
    pub fn mint_event_bridge_rule_template_group_id(&mut self) -> String {
        let id = FIRST_EVENT_BRIDGE_RULE_TEMPLATE_GROUP_ID
            + self
                .mints
                .next(|mints| &mut mints.event_bridge_rule_template_group);
        id.to_string()
    }

    /// Record `token`, as presented in `region`, as having created the group
    /// `group_id`.
    pub fn record_event_bridge_rule_template_group_token(
        &mut self,
        region: &str,
        token: &str,
        group_id: &str,
    ) {
        self.event_bridge_rule_template_group_tokens.record(
            template_group_token_key(region, token),
            group_id,
            MAX_EVENT_BRIDGE_RULE_TEMPLATE_GROUPS,
        );
    }

    /// The eventbridge rule template group a `RequestId` presented in `region`
    /// created, if that group still exists and belongs to that region.
    ///
    /// An `@idempotencyToken` is idempotent for the regional endpoint it was
    /// presented to, so the region is part of the key AND re-checked on the
    /// record: one token used in two regions creates (and resolves) a group of
    /// its own in each.
    pub fn event_bridge_rule_template_group_for_token(
        &self,
        region: &str,
        request_id: &str,
    ) -> Option<&str> {
        let id = self
            .event_bridge_rule_template_group_tokens
            .created(&template_group_token_key(region, request_id))?;
        self.event_bridge_rule_template_groups
            .get(id)
            .filter(|group| group.region == region)
            .map(|group| group.id.as_str())
    }

    /// The group called `name` in `region`, if one exists. The `Name` member is
    /// documented "Names must be unique within the scope of a resource type in a
    /// specific region", so the scope of the search is one region.
    pub fn event_bridge_rule_template_group_named(
        &self,
        region: &str,
        name: &str,
    ) -> Option<&EventBridgeRuleTemplateGroupRecord> {
        self.event_bridge_rule_template_groups
            .values()
            .find(|group| group.region == region && group.name == name)
    }

    /// The group an `Identifier` names in `region`: "Can be either be its id or
    /// current name" (the `Identifier` member's documentation), so the id is
    /// tried first and the name second.
    pub fn event_bridge_rule_template_group(
        &self,
        region: &str,
        identifier: &str,
    ) -> Option<&EventBridgeRuleTemplateGroupRecord> {
        self.event_bridge_rule_template_groups
            .get(identifier)
            .filter(|group| group.region == region)
            .or_else(|| self.event_bridge_rule_template_group_named(region, identifier))
    }

    /// The eventbridge rule template groups stored in `region`, in the order
    /// their ids were minted, beginning at the group `start` names when a
    /// `NextToken` carried a resume point.
    ///
    /// The same reading of a group store a listing of the TEMPLATES inside them
    /// already uses ([`Self::event_bridge_rule_templates_in`]): a `BTreeMap`
    /// range scan from the resume point, which is why a token naming a group
    /// deleted since the previous page resumes at the next surviving id instead
    /// of failing. Groups of other regions are invisible, for the reason
    /// [`Self::event_bridge_rule_template_group`] states.
    pub fn event_bridge_rule_template_groups_in<'a>(
        &'a self,
        region: &'a str,
        start: Option<&'a str>,
    ) -> impl Iterator<Item = &'a EventBridgeRuleTemplateGroupRecord> + 'a {
        let first = match start {
            Some(token) => ::std::ops::Bound::Included(token),
            None => ::std::ops::Bound::Unbounded,
        };
        self.event_bridge_rule_template_groups
            .range::<str, _>((first, ::std::ops::Bound::Unbounded))
            .map(|(_, group)| group)
            .filter(move |group| group.region == region)
    }

    /// Project a stored group onto the `EventBridgeRuleTemplateGroupSummary` a
    /// listing reports.
    ///
    /// Every member the summary shares with the flat group shape
    /// ([`event_bridge_rule_template_group_output`]) carries the stored value, so
    /// a listing and the `GetEventBridgeRuleTemplateGroup` beside it cannot
    /// disagree about one group. Where the two shapes differ, the summary adds
    /// `TemplateCount` — "The number of templates in a group", which is derived
    /// from the templates that exist right now
    /// ([`Self::event_bridge_rule_template_ids_in_group`]) rather than from
    /// anything recorded on the group, so it tracks every later template create
    /// and delete.
    pub fn event_bridge_rule_template_group_summary(
        &self,
        group: &EventBridgeRuleTemplateGroupRecord,
    ) -> EventBridgeRuleTemplateGroupSummary {
        EventBridgeRuleTemplateGroupSummary {
            arn: group.arn.clone(),
            created_at: ::aws_smithy_types::DateTime::from(group.created_at),
            description: group.description.clone(),
            id: group.id.clone(),
            modified_at: Some(::aws_smithy_types::DateTime::from(group.modified_at)),
            name: group.name.clone(),
            tags: Some(group.tags.clone()),
            template_count: i32::try_from(
                self.event_bridge_rule_template_ids_in_group(&group.id).len(),
            )
            .unwrap_or(i32::MAX),
        }
    }

    /// Apply the members an `UpdateEventBridgeRuleTemplateGroup` request
    /// supplied onto the group `identifier` names in `region`, and answer that
    /// group's id — or `None` when this region holds no such group.
    ///
    /// "Updates the specified eventbridge rule template group": a member the
    /// request carries replaces the stored one, a member it omits leaves that
    /// value as it was, and `ModifiedAt` becomes the instant of the call, which
    /// is when the record was last written. The identifier resolves through
    /// [`Self::event_bridge_rule_template_group`], so an update reaches its
    /// group by either form `Identifier` documents.
    pub fn update_event_bridge_rule_template_group(
        &mut self,
        region: &str,
        identifier: &str,
        description: Option<String>,
        now: SystemTime,
    ) -> Option<String> {
        let id = self
            .event_bridge_rule_template_group(region, identifier)?
            .id
            .clone();
        let group = self.event_bridge_rule_template_groups.get_mut(&id)?;
        if let Some(description) = description {
            group.description = Some(description);
        }
        group.modified_at = now;
        Some(id)
    }

    /// Remove the eventbridge rule template group `identifier` names in
    /// `region`, answering the record that was removed — or refuse.
    ///
    /// `DeleteEventBridgeRuleTemplateGroup` is documented "Deletes an eventbridge
    /// rule template group. You must detach this group from all signal maps and
    /// ensure its existing templates are moved to another group or deleted", so a
    /// group a signal map still names, or one that still holds templates, is
    /// refused and nothing is removed — reporting BOTH halves at once, so a
    /// caller learns everything it must undo from one refusal. Once removed the
    /// group is GONE from the store, so every later read of either of its
    /// identifiers misses and its name is free again.
    ///
    /// The resolution, the preconditions and the removal live together here, so a
    /// handler cannot check one of them and write past another; the handler's job
    /// is only to turn a [`TemplateGroupDeleteRefusal`] into the operation's own
    /// modelled error.
    pub fn delete_event_bridge_rule_template_group(
        &mut self,
        region: &str,
        identifier: &str,
    ) -> Result<EventBridgeRuleTemplateGroupRecord, TemplateGroupDeleteRefusal> {
        let id = self
            .event_bridge_rule_template_group(region, identifier)
            .ok_or(TemplateGroupDeleteRefusal::NotFound)?
            .id
            .clone();

        let signal_map_ids = self.signal_maps_using_event_bridge_rule_template_group(&id);
        let template_ids = self.event_bridge_rule_template_ids_in_group(&id);
        if !signal_map_ids.is_empty() || !template_ids.is_empty() {
            return Err(TemplateGroupDeleteRefusal::StillHeld {
                signal_map_ids,
                template_ids,
            });
        }

        let removed = self
            .event_bridge_rule_template_groups
            .remove(&id)
            .ok_or(TemplateGroupDeleteRefusal::NotFound)?;
        // A token never outlives the resource it minted, so a retried create
        // cannot resurrect a deleted group.
        self.event_bridge_rule_template_group_tokens.forget(&id);
        Ok(removed)
    }

    /// The ids of the signal maps that currently name the monitor template group
    /// `group_id`, through whichever of a signal map's two group lists `groups`
    /// reads.
    ///
    /// ONE walk for both group resources, so the attachment both deletes refuse
    /// on is read the same way.
    fn signal_maps_naming_group(
        &self,
        group_id: &str,
        groups: fn(&SignalMapRecord) -> &[String],
    ) -> Vec<String> {
        self.signal_maps
            .values()
            .filter(|map| groups(map).iter().any(|named| named == group_id))
            .map(|map| map.id.clone())
            .collect()
    }

    /// The ids of the signal maps that name the cloudwatch alarm template group
    /// `group_id` — the attachment `DeleteCloudWatchAlarmTemplateGroup` documents
    /// the caller must undo first ("You must detach this group from all signal
    /// maps").
    pub fn signal_maps_using_alarm_template_group(&self, group_id: &str) -> Vec<String> {
        self.signal_maps_naming_group(group_id, |map| {
            map.cloud_watch_alarm_template_group_ids.as_slice()
        })
    }

    /// The ids of the signal maps that name the eventbridge rule template group
    /// `group_id`, on the same terms as the cloudwatch alarm ones beside them.
    pub fn signal_maps_using_event_bridge_rule_template_group(
        &self,
        group_id: &str,
    ) -> Vec<String> {
        self.signal_maps_naming_group(group_id, |map| {
            map.event_bridge_rule_template_group_ids.as_slice()
        })
    }

    /// The full `Channel` description of one stored channel, or `None` when no
    /// channel has that id.
    pub fn channel_description(&self, id: &str) -> Option<Channel> {
        let channel = self.channels.get(id)?;
        Some(Channel {
            arn: Some(channel.arn.clone()),
            cdi_input_specification: channel.cdi_input_specification.clone(),
            channel_class: Some(channel.channel_class.clone()),
            destinations: Some(channel.destinations.clone()),
            // One endpoint per pipeline, each carrying the public address that
            // pipeline egresses from — allocated with the channel, so it is
            // read out of the store like every other member here.
            egress_endpoints: Some(channel.egress_endpoints()),
            encoder_settings: channel.encoder_settings.clone(),
            id: Some(channel.id.clone()),
            input_attachments: Some(channel.input_attachments.clone()),
            input_specification: channel.input_specification.clone(),
            log_level: channel.log_level.clone(),
            maintenance: channel.maintenance.clone(),
            name: channel.name.clone(),
            pipeline_details: Some(channel.pipeline_details()),
            pipelines_running_count: Some(channel.pipelines_running_count()),
            role_arn: channel.role_arn.clone(),
            state: Some(channel.state.clone()),
            tags: Some(channel.tags.clone()),
            vpc: channel.vpc.as_ref().map(described_vpc),
            anywhere_settings: channel.anywhere_settings.as_ref().map(described_anywhere),
            channel_engine_version: channel
                .channel_engine_version
                .as_deref()
                .map(|version| self.described_engine_version(version)),
            linked_channel_settings: channel
                .linked_channel_settings
                .as_ref()
                .map(|linked| self.described_linked(&channel.arn, linked)),
            channel_security_groups: Some(channel.channel_security_groups.clone()),
            inference_settings: channel.inference_settings.as_ref().map(described_inference),
        })
    }

    /// The `ChannelEngineVersionResponse` reporting the engine version a channel
    /// is pinned to: the build identifier the channel stores, and the expiry
    /// this account's engine version catalogue stores for that identifier.
    ///
    /// A channel stores only the identifier it was pinned with, so the expiry a
    /// channel-shaped read reports is read out of the same catalogue
    /// `ListVersions` reports from
    /// ([`ChannelEngineVersionCatalog::described`]) — one stored instant,
    /// however it is read.
    fn described_engine_version(
        &self,
        version: &str,
    ) -> crate::ops::types::ChannelEngineVersionResponse {
        self.channel_engine_versions.described(version)
    }

    /// The engine version this account's catalogue offers as its DEFAULT at
    /// `now` — the engine a channel that pinned none runs on.
    ///
    /// `ChannelEngineVersionRequest$Version` is "The build identifier of the
    /// engine version to use for this channel. Specify 'DEFAULT' to reset to the
    /// default version", so the model states that an account HAS a default engine
    /// version and never which build identifier it is. It is therefore DERIVED
    /// from the catalogue rather than stored beside it: the newest version still
    /// available to the account, which is the entry
    /// [`ChannelEngineVersionCatalog::available_at`] reports first (it orders the
    /// available versions newest release first), because the engine a service puts
    /// a channel on when the caller pins none is its current release. An account
    /// the service has published no engine version to has no default version, and
    /// then there is none to report.
    fn default_engine_version(
        &self,
        now: SystemTime,
    ) -> Option<crate::ops::types::ChannelEngineVersionResponse> {
        self.channel_engine_versions
            .available_at(now)
            .into_iter()
            .next()
    }

    /// The engine versions a channel's RUNNING pipelines are using, as
    /// `ChannelSummary$UsedChannelEngineVersions` reports them ("The engine
    /// version that the running pipelines are using").
    ///
    /// Derived from the two facts the store already holds about a running
    /// channel — the same two the sibling `PipelinesRunningCount` and
    /// `ChannelEngineVersion` members are derived from: which of its pipelines
    /// are up ([`ChannelRecord::healthy_pipelines`]) and which engine build it is
    /// pinned to. A channel with no pipeline up is using no engine version, so it
    /// reports none; while a pipeline is up, the engine those pipelines run is
    /// the channel's pinned build, or the account's default version
    /// ([`Self::default_engine_version`]) when the channel pinned none — and
    /// nothing at all when the channel pinned none and the account has no default
    /// version to run it on, because then there is no identifier to name.
    ///
    /// Every pipeline of one channel runs that one engine, and the member reports
    /// the versions in USE rather than one entry per pipeline, so a running
    /// channel reports exactly that one version — with the expiry the catalogue
    /// stores for it ([`Self::described_engine_version`]), the same instant
    /// `ChannelEngineVersion` and `ListVersions` report for that identifier.
    fn used_engine_versions(
        &self,
        channel: &ChannelRecord,
        now: SystemTime,
    ) -> Vec<crate::ops::types::ChannelEngineVersionResponse> {
        if channel.healthy_pipelines().is_empty() {
            return Vec::new();
        }
        channel
            .channel_engine_version
            .as_deref()
            .map(|version| self.described_engine_version(version))
            .or_else(|| self.default_engine_version(now))
            .into_iter()
            .collect()
    }

    /// The `ChannelSummary` projection of one stored channel — the same stored
    /// record as [`Self::channel_description`], minus the members the summary
    /// shape does not carry (`EncoderSettings`, `PipelineDetails`) and plus the
    /// one it carries beyond that description, `UsedChannelEngineVersions`, which
    /// [`Self::used_engine_versions`] derives from the channel's healthy
    /// pipelines and its engine version pin.
    ///
    /// `now` is the instant of the call, for the one member whose value the clock
    /// bears on: an account's default engine version is the newest of the
    /// versions still available to it, and which of them are available is a
    /// question for the clock.
    pub fn channel_summary(&self, channel: &ChannelRecord, now: SystemTime) -> ChannelSummary {
        ChannelSummary {
            arn: Some(channel.arn.clone()),
            cdi_input_specification: channel.cdi_input_specification.clone(),
            channel_class: Some(channel.channel_class.clone()),
            destinations: Some(channel.destinations.clone()),
            // The same endpoints as in `channel_description`.
            egress_endpoints: Some(channel.egress_endpoints()),
            id: Some(channel.id.clone()),
            input_attachments: Some(channel.input_attachments.clone()),
            input_specification: channel.input_specification.clone(),
            log_level: channel.log_level.clone(),
            maintenance: channel.maintenance.clone(),
            name: channel.name.clone(),
            pipelines_running_count: Some(channel.pipelines_running_count()),
            role_arn: channel.role_arn.clone(),
            state: Some(channel.state.clone()),
            tags: Some(channel.tags.clone()),
            vpc: channel.vpc.as_ref().map(described_vpc),
            anywhere_settings: channel.anywhere_settings.as_ref().map(described_anywhere),
            channel_engine_version: channel
                .channel_engine_version
                .as_deref()
                .map(|version| self.described_engine_version(version)),
            used_channel_engine_versions: Some(self.used_engine_versions(channel, now)),
            linked_channel_settings: channel
                .linked_channel_settings
                .as_ref()
                .map(|linked| self.described_linked(&channel.arn, linked)),
            channel_security_groups: Some(channel.channel_security_groups.clone()),
            inference_settings: channel.inference_settings.as_ref().map(described_inference),
        }
    }

    /// Whether the input store is full, so `CreateInput` must refuse rather than
    /// mint another input.
    pub fn inputs_are_full(&self) -> bool {
        self.inputs.len() >= MAX_INPUTS
    }

    /// The next input id, as AWS shapes them: a numeric string.
    pub fn mint_input_id(&mut self) -> String {
        (FIRST_INPUT_ID + self.mints.next(|mints| &mut mints.input)).to_string()
    }

    /// The message refusing the configuration `input` carries, or `None` when it
    /// is one this service can ingest through.
    ///
    /// ONE rule for every operation that WRITES an input's configuration
    /// (`CreateInput`, `UpdateInput`), applied to the record that WOULD be
    /// stored, so the two cannot accept configurations the other refuses:
    ///
    /// * every id the write ADDS to the configuration must resolve to a resource
    ///   this account holds — an input security group, an SdiSource, a claimed
    ///   input device. Those resources are what the input ingests through, and
    ///   each derives the inputs using it from this store
    ///   ([`Self::inputs_using_security_group`], [`Self::inputs_using_sdi_source`]),
    ///   so an id nothing minted would leave the input claiming a resource no
    ///   read of the account can show.
    /// * an input must carry the ingest list its `Type` ingests through: the
    ///   whitelist that admits the system pushing to a pushed-to type
    ///   ([`PUSH_INGEST_PROTOCOLS`]), and the source URLs a pulled-from type
    ///   pulls ([`PULL_SOURCE_INPUT_TYPES`]).
    /// * the two cross-member rules the `Vpc` member's own doc states — it
    ///   "requires setting the roleArn property on Input creation" and is "not
    ///   compatible with the inputSecurityGroups property".
    ///
    /// A VPC input and an on-premises input are the two cases the whitelist does
    /// NOT admit anything for: the former's ingest addresses "will be created in
    /// a VPC rather than with public Internet addresses", and the latter lives
    /// "in a customer network" (`InputNetworkLocation`), so neither is reached
    /// through an input security group of this account.
    ///
    /// `held` is the input as the store HOLDS it — `None` for a create, which
    /// adds every id it names. An id the held record already names is NOT
    /// resolved again: it is part of the configuration this input already
    /// claims, so a write that adds nothing to a reference list is never refused
    /// for it. Only the ids a write ADDS are resolved, which is what keeps a
    /// write that changes other members (a rename) possible after a referenced
    /// resource whose delete this service accepts while an input still uses it
    /// (an input security group, [`delete_input_security_group`]) has gone.
    pub fn input_configuration_error(
        &self,
        input: &InputRecord,
        held: Option<&InputRecord>,
    ) -> Option<String> {
        if input.vpc.is_some() {
            if !input.security_groups.is_empty() {
                return Some(VPC_WITH_SECURITY_GROUPS_MESSAGE.to_string());
            }
            if input.role_arn.as_deref().unwrap_or_default().is_empty() {
                return Some(VPC_WITHOUT_ROLE_ARN_MESSAGE.to_string());
            }
        }
        let held_groups: &[String] = held.map_or(&[], |held| &held.security_groups);
        for group in &input.security_groups {
            if held_groups.contains(group) {
                continue;
            }
            if self.live_input_security_group(group).is_none() {
                return Some(unknown_reference_message(
                    "inputSecurityGroups",
                    "input security group",
                    group,
                ));
            }
        }
        let held_sdi_sources: &[String] = held.map_or(&[], |held| &held.sdi_sources);
        for source in &input.sdi_sources {
            if held_sdi_sources.contains(source) {
                continue;
            }
            if self
                .sdi_sources
                .get(source)
                .is_none_or(SdiSourceRecord::is_deleted)
            {
                return Some(unknown_reference_message("sdiSources", "SdiSource", source));
            }
        }
        for device in &input.input_devices {
            let Some(id) = device.id.as_deref().filter(|id| !id.is_empty()) else {
                return Some(missing_reference_id_message("inputDevices"));
            };
            let held_device = held.is_some_and(|held| {
                held.input_devices
                    .iter()
                    .any(|device| device.id.as_deref() == Some(id))
            });
            if held_device {
                continue;
            }
            if self.input_device(id).is_none() {
                return Some(unknown_reference_message("inputDevices", "input device", id));
            }
        }
        let input_type = input.input_type.as_deref().unwrap_or_default();
        if input.needs_input_whitelist() && input.security_groups.is_empty() {
            return Some(required_for_input_type_message(
                "inputSecurityGroups",
                input_type,
            ));
        }
        if pulls_from_sources(input.input_type.as_deref()) && input.sources.is_empty() {
            return Some(required_for_input_type_message("sources", input_type));
        }
        None
    }

    /// Allocate the system-generated ingest address of every push destination
    /// of `id` that does not hold one yet, so the input reports the endpoint an
    /// upstream system pushes its stream to.
    ///
    /// Call this from the operations that WRITE an input's destinations
    /// (`CreateInput`, `UpdateInput`): allocating an address is a write, so no
    /// reporting operation does it.
    ///
    /// Top-up only, and only for an input whose type has a push ingest protocol
    /// ([`PUSH_INGEST_PROTOCOLS`]): an address already allocated for a
    /// destination slot stays there, which is what keeps
    /// `InputDestination$Ip` - "the system-generated static IP address of
    /// endpoint. It remains fixed for the lifetime of the input" - fixed across
    /// an update that rewrites the destinations.
    ///
    /// Every destination slot of a push input gets one, including a slot whose
    /// request named its own ON-PREM `StaticIpAddress` (which is what that slot
    /// then reports): the slot keeps an endpoint of its own, so a later update
    /// that drops the static address does not leave it without one.
    ///
    /// An address belongs to the destination SLOT, not to the stream name that
    /// occupies it: an update that shrinks the list keeps the trailing addresses,
    /// so a later update that grows it again puts the same address back on the
    /// same ingest position rather than spending a new one.
    pub fn allocate_input_ingest_addresses(&mut self, id: &str) {
        let Some(input) = self.inputs.get(id) else {
            return;
        };
        if push_ingest_protocol(input.input_type.as_deref()).is_none() {
            return;
        }
        let allocated = input.ingest_ips.len();
        let wanted = input.destinations.len();
        let fresh: Vec<String> = (allocated..wanted)
            .map(|_| self.allocate_pool_address())
            .collect();
        if let Some(input) = self.inputs.get_mut(id) {
            input.ingest_ips.extend(fresh);
        }
    }

    /// Allocate the public egress address of every pipeline of the channel `id`
    /// that does not hold one yet, so the channel reports the address its
    /// outputs originate from — `ChannelEgressEndpoint$SourceIp` ("Public IP of
    /// where a channel's output comes from"), one endpoint per pipeline.
    ///
    /// Call this from the operations that WRITE a channel's pipelines
    /// (`CreateChannel`, `UpdateChannelClass`): allocating an address is a
    /// write, so no reporting operation does it.
    ///
    /// Top-up only, out of the same pool the ingest addresses come from, so an
    /// address already allocated for a pipeline stays with that pipeline: a
    /// class change that adds a pipeline spends one more address and leaves the
    /// first pipeline egressing from the address it already reported.
    pub fn allocate_channel_egress_addresses(&mut self, id: &str) {
        let Some(channel) = self.channels.get(id) else {
            return;
        };
        let allocated = channel.egress_ips.len();
        let wanted = usize::try_from(channel.pipeline_count()).unwrap_or(0);
        let fresh: Vec<String> = (allocated..wanted)
            .map(|_| self.allocate_pool_address())
            .collect();
        if let Some(channel) = self.channels.get_mut(id) {
            channel.egress_ips.extend(fresh);
        }
    }

    /// The next address out of the emulated public address pool.
    ///
    /// The counter wraps at [`IP_POOL_LEN`], so an address could in
    /// principle be reissued - but only after 2^17 allocations, and the store
    /// holds at most [`MAX_INPUTS`] inputs, each with a caller-bounded
    /// destination list, and at most [`MAX_CHANNELS`] channels, each with at
    /// most two pipelines, so the wrap is the pool's last-resort bound rather
    /// than a path a client can reach.
    fn allocate_pool_address(&mut self) -> String {
        let offset = (self.mints.next(|mints| &mut mints.pool_ip) % u64::from(IP_POOL_LEN)) as u32;
        ::std::net::Ipv4Addr::from(IP_POOL_BASE + offset).to_string()
    }

    /// Record `token` as having created `input_id` for the create that spent it
    /// — `partner_of` is the input the new input is a partner OF, or `None` for
    /// a standalone create — under the same bounded, evicting discipline as the
    /// channel tokens.
    pub fn record_input_idempotency_token(
        &mut self,
        partner_of: Option<&str>,
        token: &str,
        input_id: &str,
    ) {
        self.input_tokens.record(
            input_token_key(partner_of, token),
            input_id,
            MAX_INPUT_IDEMPOTENCY_TOKENS,
        );
    }

    /// The input a stored `RequestId` created for the create described by
    /// `partner_of`, if that input is still one a caller may use and is still
    /// the input that create asks for.
    ///
    /// The scope is part of the key AND re-checked on the record, so a token
    /// spent on a different create resolves to nothing here: the create it
    /// named is not the one this request describes.
    ///
    /// LIVENESS is part of the lookup, through the same [`Self::live_input`] a
    /// mutation of an input resolves its id by: a delete keeps the record
    /// readable, but a token's idempotency window is over once this service has
    /// accepted the delete of the input the token minted, so the retried create
    /// mints a new input rather than answering with one the caller could
    /// neither use nor change.
    pub fn input_for_token(&self, partner_of: Option<&str>, request_id: &str) -> Option<&str> {
        let id = self
            .input_tokens
            .created(&input_token_key(partner_of, request_id))?;
        self.live_input(id)
            .filter(|input| input.partner_of.as_deref() == partner_of)
            .map(|input| input.id.as_str())
    }

    /// The channels whose `InputAttachments` name this input. One predicate, so
    /// what counts as attached cannot differ between the input's reported
    /// `AttachedChannels` and the conflict its edit answers.
    ///
    /// A channel whose delete this service has ACCEPTED attaches nothing any
    /// more — its record stays in the account, but it is being torn down, so it
    /// is no longer one of the channels the input is *currently* attached to.
    /// The cut point this module states for every derived "currently using"
    /// list.
    fn channels_attaching<'a>(
        &'a self,
        input_id: &'a str,
    ) -> impl Iterator<Item = &'a ChannelRecord> {
        self.channels
            .values()
            .filter(|channel| !channel.is_delete_accepted())
            .filter(move |channel| {
                channel
                    .input_attachments
                    .iter()
                    .any(|attachment| attachment.input_id.as_deref() == Some(input_id))
            })
    }

    /// An input's `AttachedChannels`: "A list of channel IDs that that input is
    /// attached to". Derived from the channels' own `InputAttachments`, so it
    /// tracks every channel created or deleted after this input: a channel whose
    /// delete has been accepted attaches nothing any more.
    pub fn attached_channels(&self, input_id: &str) -> Vec<String> {
        self.channels_attaching(input_id)
            .map(|channel| channel.id.clone())
            .collect()
    }

    /// The channel that stands in the way of editing this input: one whose
    /// `InputAttachments` names it and which has not stopped, reported as its id
    /// and its state.
    ///
    /// An attached channel that is still running ingests through exactly the
    /// settings an edit would replace, so the settings can only be changed once
    /// that channel has stopped ([`ChannelRecord::has_stopped`], the same notion
    /// `DeleteChannel` accepts) — the "in use" refusal `DeleteSdiSource` also
    /// makes for a source an input is using, and the condition `UpdateInput`'s
    /// modelled `ConflictException` answers.
    pub fn channel_blocking_input_edit(&self, input_id: &str) -> Option<(String, String)> {
        self.channels_attaching(input_id)
            .find(|channel| !channel.has_stopped())
            .map(|channel| (channel.id.clone(), channel.state.clone()))
    }

    /// An input's `InputPartnerIds`: "A list of IDs for all Inputs which are
    /// partners of this one". A partner pair names each other, so this is both
    /// the inputs created as a partner OF this one and, if this input is itself
    /// a partner, the input it was created for.
    fn input_partner_ids(&self, input_id: &str) -> Vec<String> {
        let mut partners: Vec<String> = self
            .inputs
            .values()
            .filter(|input| input.partner_of.as_deref() == Some(input_id))
            .map(|input| input.id.clone())
            .collect();
        if let Some(primary) = self
            .inputs
            .get(input_id)
            .and_then(|input| input.partner_of.clone())
            .filter(|primary| self.inputs.contains_key(primary))
        {
            partners.push(primary);
        }
        partners.sort();
        partners
    }

    /// The input `id` names while it is still LIVE: a record whose delete this
    /// service has not accepted.
    ///
    /// The lookup a MUTATION of an input goes through. A delete never drops the
    /// record — it keeps it in `DELETING` and then in the terminal `DELETED`, so
    /// a reader goes on reporting it — but that record is no longer a resource a
    /// caller may change, so a mutation resolves the id through here and refuses
    /// a dead one with its own modelled not-found error. Reads keep using
    /// [`Self::inputs`] / [`Self::input_description`] directly.
    pub fn live_input(&self, id: &str) -> Option<&InputRecord> {
        self.inputs
            .get(id)
            .filter(|input| !input.is_delete_accepted())
    }

    /// The channel `id` names while it is still LIVE, for the same reason as
    /// [`Self::live_input`]: a channel whose delete has been accepted is no
    /// longer a channel a caller may change (its schedule included).
    pub fn live_channel_mut(&mut self, id: &str) -> Option<&mut ChannelRecord> {
        self.channels
            .get_mut(id)
            .filter(|channel| !channel.is_delete_accepted())
    }

    /// The channel a MUTATION may change, or the refusal to answer with — the
    /// one guard every channel mutation resolves its id through, BEFORE it looks
    /// at any channel's state.
    ///
    /// An id the account holds no record for and an id whose delete this service
    /// has accepted (the record is `DELETING`, or in the terminal `DELETED`
    /// state it keeps) are ONE refusal class: neither names a channel a caller
    /// may still change, so each is answered with the operation's own modelled
    /// not-found error and nothing is written. Only the message tells them
    /// apart — `already deleted` for a tombstone the account still holds and
    /// `not found` for an id it never held.
    ///
    /// Taking this refusal first is what keeps a state conflict
    /// (`ConflictException`) for a channel that still EXISTS: a tombstone has no
    /// state a caller can conflict with, so reporting one would tell a client to
    /// retry an id that will never become startable again.
    pub fn mutable_channel(&mut self, id: &str) -> Result<&mut ChannelRecord, DeadChannel> {
        // The refusal is built from the SAME lookup that resolves the id, and
        // before the mutable borrow, so a caller reporting it (the batch
        // operations, which name the tombstone's ARN) needs no second read of
        // the store.
        let refusal = match self.channels.get(id) {
            Some(channel) if channel.is_delete_accepted() => Some(DeadChannel {
                message: already_deleted_message(&format!("Channel {id}")),
                arn: Some(channel.arn.clone()),
            }),
            Some(_) => None,
            None => Some(DeadChannel {
                message: not_found_message("Channel", id),
                arn: None,
            }),
        };
        match refusal {
            Some(refusal) => Err(refusal),
            // The lookup above already resolved the id to a live channel, so
            // this is that channel; the fallback keeps the function total
            // rather than asserting it.
            None => self.live_channel_mut(id).ok_or_else(|| DeadChannel {
                message: not_found_message("Channel", id),
                arn: None,
            }),
        }
    }

    /// The input security group `id` names while it is still LIVE: one
    /// `DeleteInputSecurityGroup` has not deleted.
    ///
    /// The lookup a request REFERENCING a group resolves through — an input
    /// configuration's `InputSecurityGroups` entry — because a deleted group is
    /// gone from `ListInputSecurityGroups` and is not a group a new resource may
    /// take a dependency on.
    ///
    /// NOT the lookup a mutation OF the group goes through: a group's delete is
    /// a SOFT delete, so the deleted group stays addressable and stays mutable
    /// (see the module docs), and `UpdateInputSecurityGroup` therefore resolves
    /// its id across the whole [`Self::input_security_groups`] store.
    pub fn live_input_security_group(&self, id: &str) -> Option<&InputSecurityGroupRecord> {
        self.input_security_groups
            .get(id)
            .filter(|group| !group.is_deleted())
    }

    /// The `InputState` of one stored input: the deletion a delete started
    /// first — `DELETING` until the clock makes it due and the terminal
    /// `DELETED` afterwards — and otherwise `ATTACHED` exactly while some
    /// channel attaches it and `DETACHED` while none does.
    pub fn input_state(&self, input: &InputRecord) -> &'static str {
        if input.is_deleted() {
            input_state::DELETED
        } else if input.is_deleting() {
            input_state::DELETING
        } else if self.attached_channels(&input.id).is_empty() {
            input_state::DETACHED
        } else {
            input_state::ATTACHED
        }
    }

    /// The full `Input` description of one stored input, or `None` when no input
    /// has that id.
    ///
    /// Every member is DERIVED from the store as it stands at the moment of the
    /// read rather than echoed from whatever the create request happened to
    /// carry: `AttachedChannels` and `State` follow the channels that attach it,
    /// and `Destinations` follows the ingest endpoints the service still holds
    /// for it ([`described_input_destinations`]), so an input whose delete has
    /// been accepted stops reporting them.
    pub fn input_description(&self, id: &str) -> Option<Input> {
        let input = self.inputs.get(id)?;
        Some(Input {
            arn: Some(input.arn.clone()),
            attached_channels: Some(self.attached_channels(id)),
            destinations: Some(described_input_destinations(input)),
            id: Some(input.id.clone()),
            input_class: Some(input.input_class().to_string()),
            input_devices: Some(input.input_devices.clone()),
            input_partner_ids: Some(self.input_partner_ids(id)),
            input_source_type: Some(input.input_source_type().to_string()),
            media_connect_flows: Some(
                input
                    .media_connect_flows
                    .iter()
                    .map(|flow| MediaConnectFlow {
                        flow_arn: flow.flow_arn.clone(),
                    })
                    .collect(),
            ),
            name: input.name.clone(),
            role_arn: input.role_arn.clone(),
            security_groups: Some(input.security_groups.clone()),
            sources: Some(input.sources.iter().map(described_input_source).collect()),
            state: Some(self.input_state(input).to_string()),
            tags: Some(input.tags.clone()),
            r#type: input.input_type.clone(),
            srt_settings: input.srt_settings.as_ref().map(described_srt_settings),
            input_network_location: Some(input.input_network_location.clone()),
            multicast_settings: input
                .multicast_settings
                .as_ref()
                .map(described_multicast_settings),
            // Reported on every Input read, with its (possibly empty) group
            // list: the structure is how a caller tells a receiver-group input's
            // streams apart, so an input configured with no receiver group
            // answers with an empty list rather than an unset member.
            smpte2110_receiver_group_settings: Some(Smpte2110ReceiverGroupSettings {
                smpte2110_receiver_groups: Some(
                    input
                        .smpte2110_receiver_group_settings
                        .as_ref()
                        .and_then(|settings| settings.smpte2110_receiver_groups.clone())
                        .unwrap_or_default(),
                ),
            }),
            sdi_sources: Some(input.sdi_sources.clone()),
            router_settings: input.router_settings.as_ref().map(described_router_settings),
        })
    }

    /// Accept the deletion of the input `id` names and answer with the input it
    /// deleted, or `None` when no input has that id.
    ///
    /// `DeleteInput` answers with the input it just deleted: the representation
    /// that input HAD when the delete was accepted, carrying the `State` the
    /// acceptance put it in. Both halves come out of the store — the snapshot is
    /// taken from the record before the deletion is written and the state is
    /// read back from the record after it — so nothing is assembled out of the
    /// request.
    ///
    /// That is why the snapshot is taken first: accepting the delete is what
    /// RELEASES the input's ingest endpoints ([`described_input_destinations`]),
    /// and a caller is told which endpoints the delete took down. Every later
    /// read of the input answers from the record itself, so it reports none of
    /// them.
    ///
    /// Idempotent, like [`begin_delete_input`]: a delete already accepted keeps
    /// the instant the first call set, and its snapshot is the torn-down input's
    /// own — endpoints already released and all.
    pub fn delete_input_description(&mut self, id: &str, now: SystemTime) -> Option<Input> {
        let mut deleted = self.input_description(id)?;
        let record = self.inputs.get_mut(id)?;
        begin_delete_input(record, now);
        let record = self.inputs.get(id)?;
        deleted.state = Some(self.input_state(record).to_string());
        Some(deleted)
    }

    /// A primary channel's `FollowingChannelArns`: the ARNs of the channels
    /// whose own follower settings name this channel as their primary. Derived
    /// from state, so it tracks every later create and delete: a channel that
    /// has reached the terminal `DELETED` state follows nothing any more.
    fn following_channel_arns(&self, primary_arn: &str) -> Vec<String> {        self.channels
            .values()
            .filter(|channel| !channel.is_deleted())
            .filter(|channel| {
                channel
                    .linked_channel_settings
                    .as_ref()
                    .and_then(|linked| linked.follower_channel_settings.as_ref())
                    .and_then(|follower| follower.primary_channel_arn.as_deref())
                    == Some(primary_arn)
            })
            .map(|channel| channel.arn.clone())
            .collect()
    }

    fn described_linked(
        &self,
        arn: &str,
        linked: &LinkedChannelSettings,
    ) -> DescribeLinkedChannelSettings {
        DescribeLinkedChannelSettings {
            follower_channel_settings: linked.follower_channel_settings.as_ref().map(|follower| {
                DescribeFollowerChannelSettings {
                    linked_channel_type: follower.linked_channel_type.clone(),
                    primary_channel_arn: follower.primary_channel_arn.clone(),
                }
            }),
            primary_channel_settings: linked.primary_channel_settings.as_ref().map(|primary| {
                DescribePrimaryChannelSettings {
                    following_channel_arns: Some(self.following_channel_arns(arn)),
                    linked_channel_type: primary.linked_channel_type.clone(),
                }
            }),
        }
    }

    /// The channel whose ARN is exactly `arn`, if this store holds one. An ARN
    /// identifies a resource globally, so a partition, region or account that
    /// differs names a resource this emulator does not hold.
    fn channel_by_arn(&self, arn: &str) -> Option<&ChannelRecord> {
        self.channels.values().find(|channel| channel.arn == arn)
    }

}

/// Project a [`SignalMapDescription`] onto one of the structurally identical
/// per-operation signal-map output shapes (`CreateSignalMapOutput`,
/// `GetSignalMapOutput`).
///
/// One macro rather than a hand-written copy per operation, so the operations
/// cannot drift from each other. The modelled maps are `HashMap`s, so the
/// `BTreeMap`s the state keeps (ordered, for deterministic iteration) are
/// converted here.
macro_rules! signal_map_description_output {
    ($out:ident, $map:expr) => {{
        let map: $crate::state::SignalMapDescription = $map;
        $out {
            arn: map.arn,
            cloud_watch_alarm_template_group_ids: map.cloud_watch_alarm_template_group_ids,
            created_at: map.created_at,
            description: map.description,
            discovery_entry_point_arn: map.discovery_entry_point_arn,
            error_message: map.error_message,
            event_bridge_rule_template_group_ids: map.event_bridge_rule_template_group_ids,
            failed_media_resource_map: map
                .failed_media_resource_map
                .map(|resources| resources.into_iter().collect()),
            id: map.id,
            last_discovered_at: map.last_discovered_at,
            last_successful_monitor_deployment: map.last_successful_monitor_deployment,
            media_resource_map: map
                .media_resource_map
                .map(|resources| resources.into_iter().collect()),
            modified_at: map.modified_at,
            monitor_changes_pending_deployment: map.monitor_changes_pending_deployment,
            monitor_deployment: map.monitor_deployment,
            name: map.name,
            status: map.status,
            tags: map.tags,
        }
    }};
}

pub(crate) use signal_map_description_output;

/// A stored instant as the model's `__timestampIso8601` type.
fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// Whether `arn` is syntactically an AWS ARN:
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`, six
/// colon-separated segments whose partition, service and resource are non-empty
/// (region and account are empty for a global service's ARNs).
pub fn is_resource_arn(arn: &str) -> bool {
    let mut segments = arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(service), Some(_region), Some(_account), Some(resource)) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return false;
    };
    !partition.is_empty() && !service.is_empty() && !resource.is_empty()
}

/// The message a request carrying more entries in one of a channel's
/// caller-chosen lists than this emulator stores is refused with: the member as
/// the wire spells it, how many entries the request carried, and the ceiling.
///
/// One helper, so `CreateChannel` and `UpdateChannel` — which store the same
/// two lists under the same two ceilings ([`MAX_INPUT_ATTACHMENTS`],
/// [`MAX_CHANNEL_DESTINATIONS`]) — cannot word that bound two ways.
pub fn too_many_channel_entries_message(member: &str, count: usize, limit: usize) -> String {
    format!("{member} carries {count} entries; this emulator accepts at most {limit}")
}

/// The message a not-found refusal carries: the resource type, the identifier
/// the request named, then `not found` — the phrasing the real service composes
/// from the request. One helper, so no two operations word it two ways.
pub fn not_found_message(resource_type: &str, identifier: &str) -> String {
    format!("{resource_type} {identifier} not found")
}

/// The message a write refused because the configuration it carries references
/// a resource this account does not hold: the member as the wire spells it, the
/// kind of resource the entry names, and the id it named.
///
/// Composed from the REQUEST, so it names what the caller must change. One
/// helper for every reference list an input configuration carries
/// (`InputSecurityGroups`, `SdiSources`, `InputDevices`), so the refusals cannot
/// be worded three ways.
pub fn unknown_reference_message(member: &str, resource_type: &str, identifier: &str) -> String {
    format!("{member} references {resource_type} {identifier}, which this account does not hold")
}

/// The message a write refused because an entry of a reference list names no id
/// at all carries: the member as the wire spells it.
pub fn missing_reference_id_message(member: &str) -> String {
    format!("every entry of {member} must name the id of a resource this account holds")
}

/// The message a write refused because a SINGLE-VALUED member that names a
/// resource carries no id the write could resolve carries: the member as the
/// wire spells it.
///
/// The scalar sibling of [`missing_reference_id_message`], which words the same
/// problem for one ENTRY of a reference list — so the two refusals say the same
/// thing about the same kind of omission.
pub fn blank_reference_id_message(member: &str) -> String {
    format!("{member} must name the id of a resource this account holds")
}

/// The message a channel-configuration write refused for a member the
/// configuration does not declare at all carries: the member as the wire spells
/// it. One helper, so `CreateChannel` and `UpdateChannel` cannot word the same
/// omission two ways.
pub fn required_channel_configuration_message(member: &str) -> String {
    format!("a channel configuration must declare {member}")
}

/// The `message` a channel-configuration refusal carries beside its
/// `ValidationErrors` entries: the entries say WHERE each problem is and what it
/// is, and this says what the refusal as a whole is about.
pub const CHANNEL_CONTAINS_ERRORS_MESSAGE: &str = "Channel contains errors.";

/// One entry of `UnprocessableEntityException$ValidationErrors`: where the
/// problem is (`ElementPath` is "Path to the source of the error") and what it
/// is.
///
/// The one constructor for every per-element problem this service reports, so
/// the schedule slice and the channel-configuration rule report a problem in the
/// same shape.
pub fn validation_problem(element_path: &str, error_message: &str) -> ValidationError {
    ValidationError {
        element_path: Some(element_path.to_string()),
        error_message: Some(error_message.to_string()),
    }
}

/// The message a write refused because the configuration leaves out the ingest
/// list the requested `InputType` ingests through carries: the member as the
/// wire spells it and the type that requires it, both out of the request.
pub fn required_for_input_type_message(member: &str, input_type: &str) -> String {
    format!("{member} is required for an input of type {input_type}")
}

/// The message a write refused for a private-VPC input that also attaches input
/// security groups carries: `Vpc` is "not compatible with the
/// inputSecurityGroups property". Nothing is interpolated — the incompatibility
/// is between two members the caller named, not about either one's value.
pub const VPC_WITH_SECURITY_GROUPS_MESSAGE: &str =
    "vpc is not compatible with the inputSecurityGroups property";

/// The message a write refused for a private-VPC input with no role carries:
/// `Vpc` "requires setting the roleArn property on Input creation".
pub const VPC_WITHOUT_ROLE_ARN_MESSAGE: &str =
    "vpc requires setting the roleArn property on Input creation";

/// Whether an `Accept` header allows a response of the media type `media_type`.
///
/// One definition for every operation that negotiates a response media type, so
/// the refusal and the response cannot disagree about what was asked for. It
/// lives beside [`not_found_message`] rather than on a resource slice, because it
/// is a property of the REQUEST and not of any resource this state holds.
///
/// The header is a comma-separated list of media ranges, each optionally carrying
/// parameters after a `;`. A range allows a type when it names that type, that
/// type's wildcard (`type/*`) or the wildcard that allows anything (`*/*`);
/// matching is case-insensitive, as media types are. A range whose quality
/// parameter is zero allows NOTHING — a zero quality is how a caller says a type
/// is not acceptable — so it is skipped, and every other parameter is ignored
/// because none of them narrows which type may be sent. A request that carries no
/// `Accept` at all asks for no particular type, so every type is allowed.
pub fn accept_allows(accept: &str, media_type: &str) -> bool {
    if accept.trim().is_empty() {
        return true;
    }
    let media_type = media_type.to_ascii_lowercase();
    let (top_level, _) = media_type.split_once('/').unwrap_or((&media_type, ""));
    let wildcard = format!("{top_level}/*");
    accept.split(',').any(|range| {
        let mut parts = range.split(';');
        let name = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
        if name != "*/*" && name != wildcard && name != media_type {
            return false;
        }
        // A quality of zero is a refusal of the range it is on, so the range
        // allows nothing; any other quality still allows the type, since this
        // service has one representation to offer rather than a choice between
        // several to rank.
        !parts.any(|parameter| {
            let (key, value) = parameter.split_once('=').unwrap_or_default();
            key.trim().eq_ignore_ascii_case("q")
                && value.trim().parse::<f64>().is_ok_and(|quality| quality <= 0.0)
        })
    })
}

/// The message a read refused for an `Accept` it cannot satisfy carries: what the
/// request asked for and the media types the operation can answer with. Built
/// from the request, so it names what the caller must change.
pub fn unacceptable_media_type_message(accept: &str, served: &[&str]) -> String {
    format!(
        "{accept} is not a media type this operation can answer with: it serves {}",
        served.join(", ")
    )
}

/// The `message` a not-found refusal carries when what the identifier failed to
/// resolve to is a resource of the EVENTBRIDGE RULE-TEMPLATE hierarchy — a rule
/// template or a rule-template group: NONE, the member is left unset.
///
/// `NotFoundException` is a structure whose single member `Message` (jsonName
/// `message`) is OPTIONAL and whose documentation is a placeholder
/// (`smithy_query(query="shape",shape="NotFoundException")`), so the model
/// states no message rule for it and the member may legitimately be absent. The
/// recorded behaviour of this hierarchy is that it IS absent: a 404 answered for
/// a rule template or a rule-template group an identifier does not resolve to
/// carries the status and the error code and no `message` member at all. So this
/// is a `None` and not an empty string — an empty string is a present member
/// whose value is empty, which is a different error document.
///
/// Every OTHER MediaLive resource family keeps composing its message with
/// [`not_found_message`]: the recorded 404s of the classic channel/input API
/// ("Channel <id> not found") and of the signal map's discovery entry point do
/// carry one, so the absence is this hierarchy's, not the service's.
///
/// ONE constant, so the ten places that report such a resource missing — its
/// own get/update/delete at both levels, the template create's and the
/// listing's group resolution, and the signal-map operations that resolve a rule
/// template group — cannot answer it two ways.
pub const EVENT_BRIDGE_RULE_TEMPLATE_NOT_FOUND_MESSAGE: Option<String> = None;

/// The message a mutation refused on an id whose delete has already been
/// accepted carries: the subject the request named, then `already deleted` —
/// the distinction the real service draws between an id that was deleted and
/// one that never existed (both answered 404). One helper for the same reason
/// as [`not_found_message`].
///
/// `subject` is how the refusal names what was deleted, which is the wording
/// each resource's recorded refusal uses: the input refusal recorded in the
/// `medialive.input-lifecycle-url-pull` behavioural check names the REQUEST
/// MEMBER and its value (`inputId=<id>`), while this service's other refusals
/// name the resource type and its id.
pub fn already_deleted_message(subject: &str) -> String {
    format!("{subject} already deleted")
}

/// How a refusal on a deleted input names it: the `InputId` member as the wire
/// spells it, then the id the request carried — composed from the request, like
/// every other message this service writes.
pub fn deleted_input_subject(input_id: &str) -> String {
    format!("inputId={input_id}")
}

/// The message a delete refused on an input a channel still attaches carries:
/// the input the REQUEST named, and that it is busy and cannot be deleted.
///
/// The template is the service's own for this rejection class, and the only
/// value it interpolates is the request's own `InputId`: which channels attach
/// the input is state the caller did not name, so it is not part of the message
/// (the attaching channels are what `DescribeInput`/`ListChannels` report).
///
/// One helper for the same reason as [`not_found_message`] — `DeleteInput` and
/// `BatchDelete` refuse the same input, so they cannot word it two ways.
pub fn busy_input_message(input_id: &str) -> String {
    format!("Input {input_id} is busy, it cannot be deleted")
}

/// The message a channel delete refused because the channel has not stopped
/// carries — the fixed sentence of that rejection class, naming the state the
/// caller must reach rather than interpolating the channel's id or its current
/// state (neither is something the caller supplied, and both are what
/// `DescribeChannel` reports).
///
/// ONE constant, so `DeleteChannel` and `BatchDelete` — which refuse the same
/// channel — cannot word the refusal two ways.
pub const CHANNEL_NOT_STOPPED_MESSAGE: &str =
    "Cannot delete a channel that has not been stopped yet. \
     Please stop the channel and try again.";

/// The message a channel stop refused on the channel's state carries: the
/// channel the request named, the state it is in, and the state a stop acts from
/// — a channel runs before it can be stopped, so every other state is refused.
///
/// ONE helper, for the same reason as [`busy_input_message`]: `StopChannel` and
/// `BatchStop` refuse the same channel, so they cannot word the refusal two
/// ways.
pub fn channel_not_running_message(channel_id: &str, state: &str) -> String {
    format!(
        "Channel {channel_id} is in state {state} and can only be stopped from {}",
        channel_state::RUNNING
    )
}

/// The message a channel start refused on the channel's state carries: the
/// channel the request named, the state it is in, and the state a start acts
/// from — a channel runs from a stopped one, so every other state is refused.
///
/// ONE helper, for the same reason as [`channel_not_running_message`]:
/// `StartChannel` and `BatchStart` refuse the same channel, so they cannot word
/// the refusal two ways.
pub fn channel_not_startable_message(channel_id: &str, state: &str) -> String {
    format!(
        "Channel {channel_id} is in state {state} and can only be started from {}",
        channel_state::IDLE
    )
}

/// The message a multiplex start refused on the multiplex's state carries: the
/// multiplex the request named and the state its record is in, which are this
/// class's two variable parts — the first taken from the request, the second
/// from the stored record.
///
/// The multiplex family words this refusal in its OWN template rather than in
/// the channel family's ([`channel_not_startable_message`]), because that is the
/// wording the service answers it with: the recorded refusal of the
/// `medialive.multiplex-start-without-programs-rejected` fixture, sent while the
/// record read `DELETING`, is "Multiplex <id> cannot be started while in
/// DELETING state". Nothing else is interpolated — the state a start does need
/// is not part of the sentence.
///
/// ONE helper, for the same reason as [`channel_not_startable_message`]:
/// `StartMultiplex` and `BatchStart` refuse the same multiplex, so they cannot
/// word the refusal two ways.
pub fn multiplex_not_startable_message(multiplex_id: &str, state: &str) -> String {
    format!("Multiplex {multiplex_id} cannot be started while in {state} state")
}

/// The message a create or a rename refused because the account already holds a
/// resource of that type under the requested `Name` carries.
///
/// Every monitor resource — the cloudwatch alarm template and its group, the
/// eventbridge rule template and its group, and the signal map — documents the
/// same uniqueness rule for `Name` ("Names must be unique within the scope of a
/// resource type in a specific region"), and the anywhere resources — the
/// cluster, the node, the channel placement group and the SdiSource — document
/// theirs ("Specify a name that is unique in the AWS account"), so the name
/// collision is ONE rejection class across all of them and this is the fixed
/// sentence it carries. Nothing is interpolated: the colliding resource's
/// internal id and the region are state the caller did not supply, and a caller
/// matching on the message would be matching on this emulator's environment.
pub const NAME_ALREADY_IN_USE_MESSAGE: &str = "Name already in use";

/// One created input, as `CreateInput` stored it plus its lifecycle state.
///
/// The REQUEST shapes are what is stored (`InputSourceRequest`,
/// `InputDestinationRequest`, …), and the `Input` response shape is projected
/// out of them by [`ComAmazonawsMedialiveMedialiveState::input_description`], so
/// nothing a request supplied is dropped and no response is built from the
/// request that happens to be in flight. A push input's ingest endpoint is
/// allocated into [`InputRecord::ingest_ips`] by
/// [`ComAmazonawsMedialiveMedialiveState::allocate_input_ingest_addresses`] when
/// its destinations are written, so the endpoint a response reports comes out of
/// the store too. Members the real service computes from infrastructure it owns
/// that this emulator does not provision (a VPC network interface, a
/// MediaConnect router output ARN) are NOT invented here.
#[derive(Debug, Clone)]
pub struct InputRecord {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    /// The `InputType` the request named, stored under a non-reserved name.
    pub input_type: Option<String>,
    pub input_network_location: String,
    pub role_arn: Option<String>,
    /// The `InputSecurityGroups` the request attached, reported as the input's
    /// `SecurityGroups`.
    pub security_groups: Vec<String>,
    pub sources: Vec<InputSourceRequest>,
    pub destinations: Vec<InputDestinationRequest>,
    /// The system-generated static ingest address this input holds for each of
    /// its destination slots - `InputDestination$Ip`, which "remains fixed for
    /// the lifetime of the input".
    ///
    /// PRIVATE: the only way in is
    /// [`ComAmazonawsMedialiveMedialiveState::allocate_input_ingest_addresses`],
    /// which owns the pool and never reassigns a slot's address.
    ingest_ips: Vec<String>,
    pub media_connect_flows: Vec<MediaConnectFlowRequest>,
    pub input_devices: Vec<InputDeviceSettings>,
    pub sdi_sources: Vec<String>,
    pub srt_settings: Option<SrtSettingsRequest>,
    pub multicast_settings: Option<MulticastSettingsCreateRequest>,
    pub smpte2110_receiver_group_settings: Option<Smpte2110ReceiverGroupSettings>,
    pub router_settings: Option<RouterSettings>,
    /// The private-VPC settings the request asked for. The `Input` shape has no
    /// member that reports them back, but they are what makes this a VPC input,
    /// so they are kept with the record rather than discarded.
    pub vpc: Option<InputVpcRequest>,
    /// The MediaConnect Router handoff settings `UpdateInput` accepts
    /// (`SpecialRouterSettings`, whose only member is the router's ARN). Kept
    /// for the same reason as `vpc`: the `Input` response shape has no member
    /// that reports a router ARN, so storing it is how the request is not
    /// silently dropped.
    pub special_router_settings: Option<SpecialRouterSettings>,
    pub tags: HashMap<String, String>,
    /// The input this one was created as a partner of (`CreatePartnerInput`),
    /// which is what makes the pair name each other in `InputPartnerIds`.
    pub partner_of: Option<String>,
    /// The instant the deletion a delete started becomes due, so `DELETING`
    /// settles into the terminal `DELETED`. `None` while no delete has been
    /// accepted for this input. Stored, never slept on.
    ///
    /// PRIVATE, so the only way in is [`begin_delete_input`] and
    /// [`InputRecord::settle`], and the only way to read it is
    /// [`InputRecord::is_deleting`].
    deletion_due: Option<SystemTime>,
    /// Whether the deletion has come due, which puts the input in the terminal
    /// `DELETED` state.
    ///
    /// A deleted input is NOT dropped from the account: `DELETED` is one of the
    /// states the model gives it (`smithy_query(query="shape",
    /// shape="InputState")`) and it is terminal, so the record stays readable in
    /// that state rather than vanishing, exactly as a deleted SdiSource does.
    deleted: bool,
}

impl InputRecord {
    /// How many ingest endpoints the create request named: the widest of the
    /// per-pipeline lists, since one entry in any of them is one pipeline's
    /// worth of ingest.
    fn ingest_endpoint_count(&self) -> usize {
        let multicast_sources = self
            .multicast_settings
            .as_ref()
            .and_then(|settings| settings.sources.as_ref())
            .map_or(0, Vec::len);
        let receiver_groups = self
            .smpte2110_receiver_group_settings
            .as_ref()
            .and_then(|settings| settings.smpte2110_receiver_groups.as_ref())
            .map_or(0, Vec::len);
        let srt_caller_sources = self
            .srt_settings
            .as_ref()
            .and_then(|settings| settings.srt_caller_sources.as_ref())
            .map_or(0, Vec::len);
        [
            self.sources.len(),
            self.destinations.len(),
            self.media_connect_flows.len(),
            self.input_devices.len(),
            self.sdi_sources.len(),
            multicast_sources,
            receiver_groups,
            srt_caller_sources,
        ]
        .into_iter()
        .max()
        .unwrap_or(0)
    }

    /// This input's `InputClass`, per the `InputClass` doc: "A standard input has
    /// two sources and a single pipeline input only has one" — so an input whose
    /// create request named exactly one ingest endpoint is `SINGLE_PIPELINE`,
    /// and any other is the two-pipeline `STANDARD` class MediaLive defaults to.
    pub fn input_class(&self) -> &'static str {
        if self.ingest_endpoint_count() == 1 {
            input_class::SINGLE_PIPELINE
        } else {
            input_class::STANDARD
        }
    }

    /// Whether `DeleteInput` has started deleting this input and the clock has
    /// not made that deletion due yet.
    pub fn is_deleting(&self) -> bool {
        self.deletion_due.is_some()
    }

    /// Whether this input's deletion has come due, which is what puts it in the
    /// terminal `DELETED` state.
    pub fn is_deleted(&self) -> bool {
        self.deleted
    }

    /// Whether a delete has been accepted for this input, in either half of the
    /// deletion lifecycle — so a repeated `DeleteInput` is idempotent rather
    /// than restarting the clock, and the id names no input a caller may still
    /// CHANGE ([`ComAmazonawsMedialiveMedialiveState::live_input`]). NOT what
    /// decides whether this input still USES the resources it references: that
    /// is cut at the terminal `DELETED` state alone
    /// ([`ComAmazonawsMedialiveMedialiveState::inputs_using_security_group`]).
    pub fn is_delete_accepted(&self) -> bool {
        self.is_deleting() || self.is_deleted()
    }

    /// Materialise the deletion the clock has already made due, answering
    /// whether THIS call is the one that settled it. Owed at the stored instant
    /// whether or not anybody called, so applying it in a reporting operation
    /// changes nothing a later caller would have seen.
    fn settle(&mut self, now: SystemTime) -> bool {
        if self.deletion_due.is_some_and(|due| now >= due) {
            self.deletion_due = None;
            self.deleted = true;
            return true;
        }
        false
    }

    /// Whether this input is ingested by an upstream system PUSHING to an
    /// endpoint of this service, which the input security groups attached to it
    /// are what admit: its `Type` has a push ingest protocol
    /// ([`PUSH_INGEST_PROTOCOLS`]) and its ingest addresses are this service's
    /// public ones — neither created inside a customer VPC (`Vpc`, whose
    /// addresses replace "public Internet addresses" and which is "not
    /// compatible with the inputSecurityGroups property") nor on premises
    /// (`InputNetworkLocation`, "On-Prem for an input in a customer network").
    pub fn needs_input_whitelist(&self) -> bool {
        push_ingest_protocol(self.input_type.as_deref()).is_some()
            && self.vpc.is_none()
            && self.input_network_location == AWS_INPUT_NETWORK_LOCATION
    }

    /// This input's `InputSourceType`: `DYNAMIC` when it is one of the types
    /// whose pull sources may carry a dynamic URL and a source URL carries the
    /// `$urlPath$` variable, otherwise `STATIC` ("By default all input sources
    /// are static", `InputSourceType`'s doc).
    pub fn input_source_type(&self) -> &'static str {
        let dynamic_capable = self
            .input_type
            .as_deref()
            .is_some_and(|input_type| DYNAMIC_CAPABLE_INPUT_TYPES.contains(&input_type));
        let dynamic_url = self.sources.iter().any(|source| {
            source
                .url
                .as_deref()
                .is_some_and(|url| url.contains(DYNAMIC_URL_VARIABLE))
        });
        if dynamic_capable && dynamic_url {
            input_source_type::DYNAMIC
        } else {
            input_source_type::STATIC
        }
    }
}

/// The key the input idempotency-token map records a token against.
///
/// Both input creates spend a `RequestId` against this ONE map, so the create
/// the token was spent on is part of the key: a standalone create and the
/// create of a partner for a given input each keep their own entry, and a
/// token spent on one of them is not a replay of the other. `partner_of` is the
/// input a partner is created for, or `None` for a standalone create. The scope
/// is re-checked on the record the token resolves to
/// ([`ComAmazonawsMedialiveMedialiveState::input_for_token`]), so the key's
/// shape carries no invariant of its own.
fn input_token_key(partner_of: Option<&str>, token: &str) -> String {
    match partner_of {
        Some(primary) => format!("partner/{primary}/{token}"),
        None => format!("input/{token}"),
    }
}

/// Build the record of a new input, with NO identity yet: every modelled member
/// starts at the empty value the request then fills in, and `id` and `arn` stay
/// empty until the create stores it.
///
/// The identity is assigned where the record is inserted, so a create refused
/// for a configuration this service cannot ingest through
/// ([`ComAmazonawsMedialiveMedialiveState::input_configuration_error`]) spends
/// no input id: the ids a client is handed are the ids of inputs that exist.
///
/// There is no transient state to owe: the real service answers `CreateInput`
/// with an input that is already `DETACHED`, so this one is usable from the
/// instant it is stored.
pub fn new_input() -> InputRecord {
    InputRecord {
        id: String::new(),
        arn: String::new(),
        name: None,
        input_type: None,
        input_network_location: DEFAULT_INPUT_NETWORK_LOCATION.to_string(),
        role_arn: None,
        security_groups: Vec::new(),
        sources: Vec::new(),
        destinations: Vec::new(),
        ingest_ips: Vec::new(),
        media_connect_flows: Vec::new(),
        input_devices: Vec::new(),
        sdi_sources: Vec::new(),
        srt_settings: None,
        multicast_settings: None,
        smpte2110_receiver_group_settings: None,
        router_settings: None,
        vpc: None,
        special_router_settings: None,
        tags: HashMap::new(),
        partner_of: None,
        deletion_due: None,
        deleted: false,
    }
}

/// Build the record of the input `CreatePartnerInput` creates as a partner of
/// `primary`: a second input, under its own minted `id` and `arn`, that ingests
/// the configuration the input it partners was created with, carrying the tags
/// the partner request supplied and naming the input it partners — which is
/// what makes the pair report each other in `InputPartnerIds`
/// ([`ComAmazonawsMedialiveMedialiveState::input_partner_ids`]).
///
/// EVERY configuration member of the partnered input is inherited, since
/// `CreatePartnerInput` carries no member to configure the partner with: its
/// `name`, `input_type`, `input_network_location`, `role_arn`,
/// `security_groups`, the per-pipeline `sources`, `destinations`,
/// `media_connect_flows`, `input_devices`, `sdi_sources`, `srt_settings`,
/// `multicast_settings`, `smpte2110_receiver_group_settings`,
/// `router_settings`, and the `vpc` and `special_router_settings` a request
/// supplied but the `Input` shape reports no member for.
///
/// What it does NOT inherit is the rest — a per-input value rather than part of
/// that configuration:
///
/// * `ingest_ips` starts empty, so
///   [`ComAmazonawsMedialiveMedialiveState::allocate_input_ingest_addresses`]
///   gives the partner an ingest endpoint of its OWN for each destination slot
///   instead of sharing the primary's allocated one — a partner exists so a
///   second source can be pushed somewhere else. A destination slot whose
///   request named its own ON-PREM `StaticIpAddress` keeps reporting that
///   address on both inputs, because that address is the CALLER's value for the
///   slot rather than one this service allocates;
/// * `tags` are the partner request's own, never a copy of the primary's;
/// * the deletion lifecycle starts fresh: the partner is a newly created input,
///   not a copy of the primary's state. Like `CreateInput`, there is no
///   transient state to owe — the input is usable from the instant it is
///   stored.
pub fn new_partner_input(
    primary: &InputRecord,
    id: String,
    arn: String,
    tags: HashMap<String, String>,
) -> InputRecord {
    InputRecord {
        id,
        arn,
        ingest_ips: Vec::new(),
        tags,
        partner_of: Some(primary.id.clone()),
        deletion_due: None,
        deleted: false,
        ..primary.clone()
    }
}

/// Delete an input: it moves into `DELETING` and owes the terminal `DELETED`
/// once [`TRANSIENT_STATE_SPAN`] has passed, which is the lifecycle the real
/// service runs (a caller waits on `State`, which is what the `input_deleted`
/// waiter polls for). The record then stays in the account in that terminal
/// state, so `DescribeInput` keeps reporting it rather than losing it.
///
/// Accepting the delete also RELEASES the ingest endpoints the input held, which
/// is why no read of it reports a destination afterwards
/// ([`described_input_destinations`]).
///
/// Idempotent: a delete already accepted keeps the instant it was accepted at,
/// so a repeated call neither restarts the clock nor revives a deleted input.
pub fn begin_delete_input(input: &mut InputRecord, now: SystemTime) {
    if input.is_delete_accepted() {
        return;
    }
    input.deletion_due = Some(now + TRANSIENT_STATE_SPAN);
}

/// `InputSourceRequest` -> `InputSource`: the same three members, so a pull
/// input reports the sources it was created with.
fn described_input_source(source: &InputSourceRequest) -> InputSource {
    InputSource {
        password_param: source.password_param.clone(),
        url: source.url.clone(),
        username: source.username.clone(),
    }
}

/// An input's `Destinations` — "A list of the destinations of the input
/// (PUSH-type)" — as they stand at the moment of the read.
///
/// A destination is an ingest ENDPOINT the service HOLDS for the input, not
/// merely configuration the caller handed it: the address behind it is
/// system-allocated and "remains fixed for the lifetime of the input"
/// (`InputDestination$Ip`), and something listens on it for the stream an
/// upstream system pushes. An input whose delete this service has accepted is
/// being torn down — `DELETING` on its way to the terminal `DELETED` — so those
/// endpoints are released and no read reports any of them: nothing ingests
/// through them any more, and a client polling `DescribeInput` to learn the
/// input has been released must not be handed an endpoint to keep pushing to.
///
/// Caller-supplied configuration (`Sources`, `SecurityGroups`, `Tags`, …) is NOT
/// released this way: it is what the record says the input was, and the deleted
/// record goes on reporting it.
///
/// The one projection that reports a released endpoint is the delete's own
/// response, which answers with the input it deleted — see
/// [`ComAmazonawsMedialiveMedialiveState::delete_input_description`].
fn described_input_destinations(input: &InputRecord) -> Vec<InputDestination> {
    if input.is_delete_accepted() {
        return Vec::new();
    }
    input
        .destinations
        .iter()
        .enumerate()
        .map(|(index, destination)| described_input_destination(input, index, destination))
        .collect()
}

/// `InputDestinationRequest` -> `InputDestination`, for the destination at
/// `index` of `input`'s destination list.
///
/// `Ip` is the request's `StaticIpAddress` when it named one (the address an
/// ON-PREM push input takes on the customer network), otherwise the
/// system-generated address allocated for that destination slot. `Port` and
/// `Url` are the ingest endpoint of the input type's push protocol, so a client
/// creating a push input learns where to push the stream; an input type with no
/// push protocol ([`PUSH_INGEST_PROTOCOLS`]) reports neither.
///
/// The three endpoint members are reported together or not at all: without an
/// address there is no endpoint to announce, so no port is claimed for one
/// either.
///
/// `Vpc` describes the network interface the real service provisions in the
/// customer VPC, which this emulator does not, so it claims none of it.
fn described_input_destination(
    input: &InputRecord,
    index: usize,
    destination: &InputDestinationRequest,
) -> InputDestination {
    let ip = destination
        .static_ip_address
        .clone()
        .filter(|address| !address.is_empty())
        .or_else(|| input.ingest_ips.get(index).cloned());
    let endpoint = push_ingest_protocol(input.input_type.as_deref()).zip(ip.as_deref());
    InputDestination {
        port: endpoint.map(|(protocol, _)| protocol.port.to_string()),
        url: endpoint
            .map(|(protocol, ip)| protocol.endpoint_url(ip, destination.stream_name.as_deref())),
        ip,
        vpc: None,
        network: destination.network.clone(),
        // A list member of the projection is reported as present and empty
        // rather than unset when the input has no routes, like every other list
        // in the `Input` shape.
        network_routes: Some(
            destination
                .network_routes
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|route| InputDestinationRoute {
                    cidr: route.cidr.clone(),
                    gateway: route.gateway.clone(),
                })
                .collect(),
        ),
    }
}

/// `SrtSettingsRequest` -> `SrtSettings`.
fn described_srt_settings(settings: &SrtSettingsRequest) -> SrtSettings {
    SrtSettings {
        srt_caller_sources: settings.srt_caller_sources.as_ref().map(|sources| {
            sources
                .iter()
                .map(|source| SrtCallerSource {
                    decryption: source.decryption.as_ref().map(|decryption| {
                        SrtCallerDecryption {
                            algorithm: decryption.algorithm.clone(),
                            passphrase_secret_arn: decryption.passphrase_secret_arn.clone(),
                        }
                    }),
                    minimum_latency: source.minimum_latency,
                    srt_listener_address: source.srt_listener_address.clone(),
                    srt_listener_port: source.srt_listener_port.clone(),
                    stream_id: source.stream_id.clone(),
                })
                .collect()
        }),
        srt_listener_settings: settings.srt_listener_settings.as_ref().map(|listener| {
            SrtListenerSettings {
                decryption: Some(SrtListenerDecryption {
                    algorithm: listener.decryption.algorithm.clone(),
                    passphrase_secret_arn: listener.decryption.passphrase_secret_arn.clone(),
                }),
                minimum_latency: Some(listener.minimum_latency),
                stream_id: listener.stream_id.clone(),
            }
        }),
    }
}

/// `MulticastSettingsCreateRequest` -> `MulticastSettings`.
fn described_multicast_settings(
    settings: &MulticastSettingsCreateRequest,
) -> MulticastSettings {
    MulticastSettings {
        sources: settings.sources.as_ref().map(|sources| {
            sources
                .iter()
                .map(|source| MulticastSource {
                    source_ip: source.source_ip.clone(),
                    url: source.url.clone(),
                })
                .collect()
        }),
    }
}

/// `RouterSettings` (request) -> `RouterInputSettings` (response).
///
/// `RouterOutputArn` names the MediaConnect router output the real service
/// associates with the input, which this emulator does not create, so it stays
/// absent rather than being invented.
fn described_router_settings(settings: &RouterSettings) -> RouterInputSettings {
    RouterInputSettings {
        destinations: settings.destinations.as_ref().map(|destinations| {
            destinations
                .iter()
                .map(|destination| RouterDestination {
                    availability_zone_name: Some(destination.availability_zone_name.clone()),
                    router_output_arn: None,
                })
                .collect()
        }),
        encryption_type: settings.encryption_type.clone(),
        secret_arn: settings.secret_arn.clone(),
    }
}

/// `VpcOutputSettings` (request) -> `VpcOutputSettingsDescription` (response).
///
/// `AvailabilityZones` and `NetworkInterfaceIds` are left absent: the real
/// service fills them from the ENIs it allocates, which this emulator does not
/// create, so reporting values for them would be fabrication.
fn described_vpc(vpc: &VpcOutputSettings) -> VpcOutputSettingsDescription {
    VpcOutputSettingsDescription {
        availability_zones: None,
        network_interface_ids: None,
        security_group_ids: vpc.security_group_ids.clone(),
        subnet_ids: Some(vpc.subnet_ids.clone()),
    }
}

fn described_anywhere(anywhere: &AnywhereSettings) -> DescribeAnywhereSettings {
    DescribeAnywhereSettings {
        channel_placement_group_id: anywhere.channel_placement_group_id.clone(),
        cluster_id: anywhere.cluster_id.clone(),
    }
}

fn described_inference(inference: &InferenceSettings) -> DescribeInferenceSettings {
    DescribeInferenceSettings {
        feed_arn: inference.feed_arn.clone(),
        audio_feed_inputs: inference.audio_feed_inputs.clone(),
        enrichment_methods: inference.enrichment_methods.clone(),
    }
}

/// The days `MaintenanceDay` gives the service to choose a channel's
/// maintenance window from, in the model's own order
/// (`smithy_query(query="shape", shape="MaintenanceDay")`: MONDAY | TUESDAY |
/// WEDNESDAY | THURSDAY | FRIDAY | SATURDAY | SUNDAY).
const MAINTENANCE_DAYS: [&str; 7] = [
    "MONDAY",
    "TUESDAY",
    "WEDNESDAY",
    "THURSDAY",
    "FRIDAY",
    "SATURDAY",
    "SUNDAY",
];

/// The hours a maintenance start time can name, which is what makes the start
/// time a whole hour of the day ("Choose the hour that maintenance will
/// start").
const MAINTENANCE_HOURS: usize = 24;

/// The maintenance window the service selects for a channel.
///
/// EVERY channel has one selected: the response member is a *status* whose day
/// is "The currently selected maintenance day" and whose time is "The currently
/// selected maintenance start time", while the create setting that would choose
/// them is optional — so the selection is made here, when the channel is
/// created, rather than left absent until some caller writes one.
///
/// Derived from the channel's own minted id, so the window is stable for the
/// channel's whole lifetime (nothing about the request or the clock enters it)
/// and the windows of an account's channels are spread over the week and over
/// the hours of the day instead of all falling together. The start time is a
/// whole hour, the only form the member accepts.
///
/// `MaintenanceDeadline` ("Maintenance is required by the displayed date and
/// time") and `MaintenanceScheduledDate` ("The currently scheduled maintenance
/// date and time") are assigned by the real service's maintenance scheduler,
/// which this emulator does not run, so they stay absent: no maintenance has
/// been scheduled for or required of this channel.
fn assigned_maintenance_window(id: &str) -> MaintenanceStatus {
    let seed = usize::try_from(id.parse::<u64>().unwrap_or_default()).unwrap_or_default();
    let hour = seed % MAINTENANCE_HOURS;
    MaintenanceStatus {
        maintenance_day: Some(MAINTENANCE_DAYS[seed % MAINTENANCE_DAYS.len()].to_string()),
        maintenance_deadline: None,
        maintenance_scheduled_date: None,
        maintenance_start_time: Some(format!("{hour:02}:00")),
    }
}

/// Apply the day and start time a `CreateChannel` request chose to the window
/// the service assigned the channel.
///
/// Member by member, because each of the two is optional and documented on its
/// own ("Choose one day of the week for maintenance", "Choose the hour that
/// maintenance will start"): a request that names one leaves the other at the
/// service's selection, so the channel carries a complete window either way.
pub fn choose_maintenance_window(
    stored: &mut Option<MaintenanceStatus>,
    chosen: MaintenanceCreateSettings,
) {
    let stored = stored.get_or_insert_with(MaintenanceStatus::default);
    if chosen.maintenance_day.is_some() {
        stored.maintenance_day = chosen.maintenance_day;
    }
    if chosen.maintenance_start_time.is_some() {
        stored.maintenance_start_time = chosen.maintenance_start_time;
    }
}

/// Build a new channel record from a `CreateChannel` request's values.
///
/// Every member is the value the request supplied; the lifecycle members and the
/// service-selected maintenance window are the only computed ones. The channel
/// starts in `CREATING` and owes `IDLE`, which is what the real service does.
pub fn new_channel(
    id: String,
    arn: String,
    channel_class: String,
    now: SystemTime,
) -> ChannelRecord {
    let maintenance = assigned_maintenance_window(&id);
    let mut channel = ChannelRecord {
        id,
        arn,
        state: String::new(),
        channel_class,
        name: None,
        cdi_input_specification: None,
        destinations: Vec::new(),
        encoder_settings: None,
        input_attachments: Vec::new(),
        input_specification: None,
        log_level: None,
        maintenance: Some(maintenance),
        role_arn: None,
        tags: HashMap::new(),
        channel_security_groups: Vec::new(),
        anywhere_settings: None,
        linked_channel_settings: None,
        inference_settings: None,
        vpc: None,
        channel_engine_version: None,
        egress_ips: Vec::new(),
        special_router_settings: None,
        restarting_pipelines: Vec::new(),
        schedule: ChannelSchedule::default(),
        alerts: ChannelAlerts::default(),
        thumbnails: Vec::new(),
        transition: None,
        state_since: now,
    };
    channel.begin_transition(channel_state::CREATING, Settled::Idle, now);
    channel
}

/// Move a channel from `IDLE` into `STARTING`, owing `RUNNING`.
pub fn begin_start(channel: &mut ChannelRecord, now: SystemTime) {
    channel.begin_transition(channel_state::STARTING, Settled::Running, now);
}

/// Move a stopped channel into `UPDATING`, owing `IDLE` again.
///
/// `UPDATING` is the `ChannelState` value for a channel whose configuration is
/// being applied, and applying it leaves the channel stopped, so the state the
/// clock owes is the `IDLE` the update started from.
pub fn begin_update(channel: &mut ChannelRecord, now: SystemTime) {
    channel.begin_transition(channel_state::UPDATING, Settled::Idle, now);
}

/// Move a running channel into `STOPPING`, owing `IDLE`.
pub fn begin_stop(channel: &mut ChannelRecord, now: SystemTime) {
    channel.begin_transition(channel_state::STOPPING, Settled::Idle, now);
}

/// Move a channel into `DELETING`, owing the terminal `DELETED` state it then
/// keeps.
pub fn begin_delete(channel: &mut ChannelRecord, now: SystemTime) {
    channel.begin_transition(channel_state::DELETING, Settled::Deleted, now);
}

/// Take `pipelines` down for a restart on a running channel: it moves into
/// `RECOVERING` — the `ChannelState` value for a channel whose pipelines are
/// coming back up — and owes `RUNNING` with every pipeline healthy again once
/// the restart completes.
///
/// `pipelines` are `PipelineDetail$PipelineId` numbers, each one the caller's
/// `RestartChannelPipelines` request asked for; the channel reports them as not
/// healthy for as long as it is `RECOVERING`.
pub fn begin_pipeline_restart(
    channel: &mut ChannelRecord,
    pipelines: Vec<i32>,
    now: SystemTime,
) {
    channel.restarting_pipelines = pipelines;
    channel.begin_transition(channel_state::RECOVERING, Settled::Running, now);
}

/// The `State` of `source`, whose attached inputs are `inputs`.
///
/// `DELETED` once `DeleteSdiSource` has deleted it — the terminal state, which a
/// deleted source keeps. Otherwise `IN_USE` when an input is attached to it and
/// `IDLE` when none is, which is the `State` member's own definition ("whether
/// the SDI source is attached to an SDI input (IN_USE) or not (IDLE)"). Derived
/// from the store on every read, never written at create time.
fn sdi_source_state_for(source: &SdiSourceRecord, inputs: &[String]) -> &'static str {
    if source.is_deleted() {
        sdi_source_state::DELETED
    } else if inputs.is_empty() {
        sdi_source_state::IDLE
    } else {
        sdi_source_state::IN_USE
    }
}

/// The `State` of `group`, whose current users are `inputs` and `channels`.
///
/// `DELETED` once `DeleteInputSecurityGroup` has deleted it — the terminal
/// state, which a deleted group keeps, so a client polling for it sees it and
/// one that treats `IDLE` as "available" cannot attach a deleted group.
/// Otherwise the state the group's users imply
/// ([`input_security_group_usage_state`]). Derived from the store on every read,
/// never written at create time.
fn input_security_group_state_for(
    group: &InputSecurityGroupRecord,
    inputs: &[String],
    channels: &[String],
) -> &'static str {
    if group.is_deleted() {
        input_security_group_state::DELETED
    } else {
        input_security_group_usage_state(inputs, channels)
    }
}

/// The `State` the group's users alone imply: `IN_USE` while some resource
/// references it and `IDLE` while none does, which is what
/// `InputSecurityGroup$State` reports ("The current state of the Input Security
/// Group").
fn input_security_group_usage_state(inputs: &[String], channels: &[String]) -> &'static str {
    if inputs.is_empty() && channels.is_empty() {
        input_security_group_state::IDLE
    } else {
        input_security_group_state::IN_USE
    }
}

/// Build a new SdiSource record from a `CreateSdiSource` request's id and ARN.
///
/// The caller stores the request's `Name`, `Mode`, `Type` and `Tags` on the
/// record it gets back. There is no lifecycle transition to owe: a created
/// SdiSource is `IDLE` immediately, because nothing is attached to it yet.
pub fn new_sdi_source(id: String, arn: String) -> SdiSourceRecord {
    SdiSourceRecord {
        id,
        arn,
        name: None,
        mode: None,
        r#type: None,
        tags: HashMap::new(),
        deleted: false,
    }
}

/// Build a new eventbridge rule template group record.
///
/// `name` is the value the request supplied; `created_at` and `modified_at` are
/// the create instant, because a group has not been modified since the call
/// that created it. The caller then stores the request's optional `Description`
/// and `Tags` on the record. There is no lifecycle transition to owe: AWS
/// returns a created group ready.
pub fn new_event_bridge_rule_template_group(
    id: String,
    arn: String,
    region: String,
    name: String,
    now: SystemTime,
) -> EventBridgeRuleTemplateGroupRecord {
    EventBridgeRuleTemplateGroupRecord {
        id,
        arn,
        region,
        name,
        description: None,
        tags: HashMap::new(),
        created_at: now,
        modified_at: now,
    }
}

/// The message for the failures that are not about the request: this service was
/// mounted on an engine that holds no store for it, or the store keeps no
/// partition for the region the request was signed for and has no room for
/// another ([`MAX_RESOURCE_REGIONS`]). One constant, so the service's operations
/// cannot word it several ways.
pub const NO_STORE: &str = "medialive service state is unavailable";

/// Project a `Channel` onto one of the structurally identical per-operation
/// channel-description output shapes (`DescribeChannelOutput`,
/// `StartChannelOutput`, `StopChannelOutput`, `DeleteChannelOutput`,
/// `RestartChannelPipelinesOutput`).
///
/// One macro rather than five hand-written copies of the same 23-member move, so
/// the operations cannot drift from each other. A shape that carries a member
/// beyond the 23 (`RestartChannelPipelinesOutput$MaintenanceStatus`) passes it as
/// a trailing `member: value` pair, so every member of every one of these shapes
/// is still named exactly once and adding one to the model cannot be defaulted in
/// silently.
macro_rules! channel_description_output {
    ($out:ident, $channel:expr $(, $extra:ident : $value:expr)* $(,)?) => {{
        let channel: $crate::ops::types::Channel = $channel;
        $out {
            arn: channel.arn,
            cdi_input_specification: channel.cdi_input_specification,
            channel_class: channel.channel_class,
            destinations: channel.destinations,
            egress_endpoints: channel.egress_endpoints,
            encoder_settings: channel.encoder_settings,
            id: channel.id,
            input_attachments: channel.input_attachments,
            input_specification: channel.input_specification,
            log_level: channel.log_level,
            maintenance: channel.maintenance,
            name: channel.name,
            pipeline_details: channel.pipeline_details,
            pipelines_running_count: channel.pipelines_running_count,
            role_arn: channel.role_arn,
            state: channel.state,
            tags: channel.tags,
            vpc: channel.vpc,
            anywhere_settings: channel.anywhere_settings,
            channel_engine_version: channel.channel_engine_version,
            linked_channel_settings: channel.linked_channel_settings,
            channel_security_groups: channel.channel_security_groups,
            inference_settings: channel.inference_settings,
            $($extra: $value,)*
        }
    }};
}

pub(crate) use channel_description_output;

/// Project an `Input` onto one of the structurally identical per-operation
/// input-description output shapes (`DescribeInputOutput` today; the others when
/// their own issues land).
///
/// `DeleteInput` serialises through `DescribeInputOutput` itself — see the note
/// on that handler — so the deleted input it answers with is the same projection
/// a later `DescribeInput` reports.
///
/// One macro rather than a hand-written copy of the same 22-member move per
/// operation, so those operations cannot drift from each other.
macro_rules! input_description_output {
    ($out:ident, $input:expr) => {{
        let input: $crate::ops::types::Input = $input;
        $out {
            arn: input.arn,
            attached_channels: input.attached_channels,
            destinations: input.destinations,
            id: input.id,
            input_class: input.input_class,
            input_devices: input.input_devices,
            input_partner_ids: input.input_partner_ids,
            input_source_type: input.input_source_type,
            media_connect_flows: input.media_connect_flows,
            name: input.name,
            role_arn: input.role_arn,
            security_groups: input.security_groups,
            sources: input.sources,
            state: input.state,
            tags: input.tags,
            r#type: input.r#type,
            srt_settings: input.srt_settings,
            input_network_location: input.input_network_location,
            multicast_settings: input.multicast_settings,
            smpte2110_receiver_group_settings: input.smpte2110_receiver_group_settings,
            sdi_sources: input.sdi_sources,
            router_settings: input.router_settings,
        }
    }};
}

pub(crate) use input_description_output;

/// Project an `InputSecurityGroup` onto one of the structurally identical
/// per-operation FLAT group output shapes (`DescribeInputSecurityGroupOutput`;
/// `DeleteInputSecurityGroup` serialises through that same shape — see the note
/// on that handler).
///
/// One macro rather than a hand-written copy of the same seven-member move per
/// operation, so the operations that report a group flat cannot drift from each
/// other. The operations whose model WRAPS the group in a `SecurityGroup` member
/// (`CreateInputSecurityGroup`, `UpdateInputSecurityGroup`) use the
/// `InputSecurityGroup` shape itself and so need no projection.
macro_rules! input_security_group_description_output {
    ($out:ident, $group:expr) => {{
        let group: $crate::ops::types::InputSecurityGroup = $group;
        $out {
            arn: group.arn,
            id: group.id,
            inputs: group.inputs,
            state: group.state,
            tags: group.tags,
            whitelist_rules: group.whitelist_rules,
            channels: group.channels,
        }
    }};
}

pub(crate) use input_security_group_description_output;

/// Project a stored [`EventBridgeRuleTemplateGroupRecord`] onto one of the
/// structurally identical per-operation group output shapes
/// (`CreateEventBridgeRuleTemplateGroupOutput`,
/// `GetEventBridgeRuleTemplateGroupOutput` and
/// `UpdateEventBridgeRuleTemplateGroupOutput`).
///
/// One macro rather than a hand-written copy per operation, so the operations
/// that report a group cannot drift from each other. `Tags` is always reported,
/// as the map it is modelled as; see [`template_group_output`].
macro_rules! event_bridge_rule_template_group_output {
    ($out:ident, $group:expr) => {
        $crate::state::template_group_output!(
            $out,
            $crate::state::EventBridgeRuleTemplateGroupRecord,
            $group
        )
    };
}

pub(crate) use event_bridge_rule_template_group_output;

/// The ONE projection of a stored template-group record onto a group output
/// shape, parameterised by the record type.
///
/// Every monitor template group the service models carries the same members, so
/// the per-resource macros above and below delegate here rather than each
/// carrying its own copy of the projection, which could then drift.
///
/// `Tags` reports the group's tag set WHATEVER it holds — an untagged group
/// reports the empty map, not an absent member. The member is modelled as a
/// `TagMap`, and a resource that carries no tags carries zero tags rather than
/// an unknown number of them, so a caller that iterates the map sees an empty
/// map instead of a null it has to special-case. It is also how this service's
/// own signal maps already report their tags
/// ([`ComAmazonawsMedialiveMedialiveState::signal_map_description`]), so one
/// resource of this service cannot answer "no tags" differently from another.
macro_rules! template_group_output {
    ($out:ident, $record:ty, $group:expr) => {{
        let group: &$record = $group;
        $out {
            arn: Some(group.arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(group.created_at)),
            description: group.description.clone(),
            id: Some(group.id.clone()),
            modified_at: Some(::aws_smithy_types::DateTime::from(group.modified_at)),
            name: Some(group.name.clone()),
            tags: Some(group.tags.clone()),
        }
    }};
}

pub(crate) use template_group_output;

/// The REGION slice: the store the engine holds, the per-region partition of
/// resources a handler is handed, the ceiling on how many of those partitions
/// the store keeps, and the decode that says which region a request is for — in
/// one module of its own for the same reason as [`account_configuration`]. What
/// stays in this file is the part the rest of the state needs: the records
/// themselves, and the [`IdMints`] every partition shares.
pub mod regions;

/// The SIGNAL MAP slice, in one module of its own for the same reason as
/// [`batch_results`]: everything a signal map is and does, including the
/// media-resource map a discovery walks. Re-exported below, so a handler's
/// `crate::state::…` path resolves whichever file the item lives in.
mod signal_maps;
/// The TAGGING slice: the tag bounds, the reserved key namespace and the reads
/// and writes of one resource's tags.
mod tags;
/// The NETWORK slice: the network record, its pools and routes, and its
/// projections.
mod networks;
/// The SDI SOURCE slice: the SdiSource record, which inputs use one, and its
/// projections.
mod sdi_sources;

pub use networks::*;
pub use sdi_sources::*;
pub use signal_maps::*;
pub use tags::*;

pub use regions::{
    request_region, service_state, service_state_in, service_store,
    ComAmazonawsMedialiveMedialiveStore, RegionState, MAX_RESOURCE_REGIONS,
};

/// The counters this service mints every resource id out of, one per kind of id.
///
/// Held behind one lock rather than one per counter, so a mint is a single
/// critical section and the whole set can be handed to every region partition of
/// a store as one shared value. `Mints` itself is the counters; [`Self::next`] is
/// the only way to move one.
#[derive(Debug, Default)]
pub struct IdMints(::std::sync::Mutex<Mints>);

impl IdMints {
    /// The next value of the counter `pick` names — the offset the caller adds
    /// its own kind's first id to — which no later call hands out again.
    ///
    /// Recovers from a poisoned lock for the same reason the state guard does:
    /// one panicking handler must not stop every later request from being able
    /// to mint an id.
    fn next(&self, pick: fn(&mut Mints) -> &mut u64) -> u64 {
        let mut mints = self
            .0
            .lock()
            .unwrap_or_else(::std::sync::PoisonError::into_inner);
        let counter = pick(&mut mints);
        let minted = *counter;
        *counter += 1;
        minted
    }
}

/// One counter per kind of id this service hands out, each the number of ids of
/// that kind minted so far: the id itself is that offset past the kind's first
/// id (`FIRST_CHANNEL_ID` and the constants beside it), which is what keeps one
/// kind's ids out of another's range.
#[derive(Debug, Default)]
struct Mints {
    /// Behind [`ComAmazonawsMedialiveMedialiveState::mint_channel_id`].
    channel: u64,
    /// Behind [`ComAmazonawsMedialiveMedialiveState::mint_input_id`].
    input: u64,
    /// Behind
    /// [`ComAmazonawsMedialiveMedialiveState::mint_input_security_group_id`].
    input_security_group: u64,
    /// Behind [`ComAmazonawsMedialiveMedialiveState::mint_sdi_source_id`].
    sdi_source: u64,
    /// Behind [`ComAmazonawsMedialiveMedialiveState::mint_network_id`].
    network: u64,
    /// Behind [`ComAmazonawsMedialiveMedialiveState::mint_signal_map_id`].
    signal_map: u64,
    /// Behind the multiplex slice's `mint_multiplex_id`.
    multiplex: u64,
    /// Behind the alert ids the multiplex slice's `apply_alerts` hands out:
    /// `MultiplexAlert$Id` is "The unique ID for this alert instance", so two
    /// multiplexes cannot raise an alert under the same id.
    multiplex_alert: u64,
    /// Behind the cluster slice's `mint_cluster_id`.
    cluster: u64,
    /// Behind the channel placement group slice's
    /// `mint_channel_placement_group_id`.
    channel_placement_group: u64,
    /// Behind the node slice's `mint_node_id`.
    node: u64,
    /// Behind the registration script slice's
    /// `mint_node_registration_script_id`.
    node_registration_script: u64,
    /// Behind the reservation slice's `mint_reservation_id`.
    reservation: u64,
    /// Behind the alarm template group slice's `mint_alarm_template_group_id`.
    alarm_template_group: u64,
    /// Behind the alarm template slice's `mint_alarm_template_id`.
    alarm_template: u64,
    /// Behind
    /// [`ComAmazonawsMedialiveMedialiveState::mint_event_bridge_rule_template_group_id`].
    event_bridge_rule_template_group: u64,
    /// Behind the eventbridge rule template slice's
    /// `mint_event_bridge_rule_template_id`.
    event_bridge_rule_template: u64,
    /// The offset into the emulated public address pool
    /// ([`ComAmazonawsMedialiveMedialiveState::allocate_pool_address`]), so no
    /// two endpoints this service reports — a push input's ingest address, a
    /// channel pipeline's egress address — are the same, in one region or
    /// across two.
    pool_ip: u64,
}

// ---------------------------------------------------------------------------
// What the cloudwatch alarm template group slice leaves in this file: the token
// key below, which both group resources share. The slice itself is the
// `alarm_template_groups` module declared at the top of this file, and its store
// is the `alarm_template_groups` member of
// `ComAmazonawsMedialiveMedialiveState` above.
// ---------------------------------------------------------------------------

/// The key a monitor template token map uses — both group resources and the
/// eventbridge rule templates inside them.
///
/// A `RequestId` is idempotent for the regional endpoint it was presented to, so
/// the region is part of the key: two regions using the same token each keep
/// their own entry instead of overwriting one another's. The region is re-checked
/// on the record the token resolves to, so the key's shape carries no invariant
/// of its own.
fn template_group_token_key(region: &str, token: &str) -> String {
    format!("{region}/{token}")
}
