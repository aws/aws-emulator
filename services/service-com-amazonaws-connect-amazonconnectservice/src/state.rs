// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connect-amazonconnectservice service.
//!
//! Holds Amazon Connect's primary resource loop — an instance and the queues that
//! live inside it — plus the small helpers every handler of that loop needs
//! (identifier minting, ARN construction, pagination tokens, and the record ->
//! modelled-shape projections). The helpers live here rather than in one operation's
//! file because `src/ops/<op>.rs` is per-operation: a projection that `DescribeQueue`
//! and `ListQueues` both need has to be reachable from both.
//!
//! The claimable phone-number inventory beside it works the same way:
//! [`inventory_phone_number`] derives the numbers one country and type are offered,
//! [`ComAmazonawsConnectAmazonconnectserviceState::available_phone_numbers`]
//! subtracts the ones already claimed — which is what
//! `SearchAvailablePhoneNumbers` advertises — and
//! [`ComAmazonawsConnectAmazonconnectserviceState::phone_numbers`] is the single
//! record set `ClaimPhoneNumber` writes, `DescribePhoneNumber` projects and
//! `ReleasePhoneNumber` removes.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// The attached-files configuration of an instance's attachment scopes: one concern of
/// its own, so its own file, re-exported so a handler reaches it through
/// `crate::state` like everything else here.
mod attached_files;
pub use attached_files::{
    requested_extensions, AttachedFilesConfigurationRecord, DEFAULT_MAXIMUM_SIZE_LIMIT_IN_BYTES,
};

/// The chat half of the instance: what `StartChatContact` records here and what it
/// hands the Connect Participant Service. Its own file because it is one concern of
/// its own, and it is re-exported so a handler reaches it through `crate::state` like
/// everything else here.
mod chat_contacts;
pub use chat_contacts::{
    attributes_problem, chat_duration, chat_session_for, chat_session_state,
    connection_token_expiry, publish_chat_attachment_sharing, ChatContactRecord, ChatStreaming,
    ATTACHMENTS_STORAGE_RESOURCE_TYPE, CHAT_CONTACTS_PER_INSTANCE_QUOTA,
    CHAT_DURATION_DEFAULT_MINUTES, CHAT_MESSAGE_CONTENT_TYPES, MAX_ATTRIBUTES_BYTES,
    REQUIRED_MESSAGING_CONTENT_TYPE, SUPPORTED_MESSAGING_CONTENT_TYPES,
};

/// The instance's feature toggles: what `UpdateInstanceAttribute` writes,
/// `DescribeInstanceAttribute` reads back and `ListInstanceAttributes` enumerates. Its own
/// file because it is one concern of its own, and it is re-exported so a handler reaches it
/// through `crate::state` like everything else here.
mod instance_attributes;
pub use instance_attributes::{
    default_instance_attribute_value, instance_attribute_toggle, instance_attribute_type_refusal,
    is_instance_attribute_toggle, is_instance_attribute_type, ATTRIBUTES_ENABLED_BY_DEFAULT,
    INBOUND_CALLS_ATTRIBUTE, INSTANCE_ATTRIBUTE_DISABLED, INSTANCE_ATTRIBUTE_ENABLED,
    INSTANCE_ATTRIBUTE_TYPES_BEYOND_THE_MODEL, INSTANCE_ATTRIBUTE_TYPE_PARAMETER,
    INSTANCE_ATTRIBUTE_VALUE_PARAMETER, LIST_INSTANCE_ATTRIBUTES_DEFAULT_PAGE_SIZE,
    LIST_INSTANCE_ATTRIBUTES_MAX_RESULTS_MIN, OUTBOUND_CALLS_ATTRIBUTE,
};

/// The tagging plane's lookup from a resource ARN to the tag map that resource carries:
/// one concern of its own, so its own file. Most of it extends
/// [`ComAmazonawsConnectAmazonconnectserviceState`]; the bound a tag map is held to, and
/// how the growth of one is measured, are re-exported so a handler reaches them through
/// `crate::state` like everything else here.
mod tagging;
pub use tagging::{tags_growth, TAGS_PARAMETER, TAGS_PER_RESOURCE_MAXIMUM};

/// How this service words a refusal, and the identifier forms a path label may take:
/// one concern of its own, so its own file, re-exported so a handler reaches it through
/// `crate::state` like everything else here.
mod refusals;
pub use refusals::{
    instance_id_is_not_valid, instance_label_refusal, instance_not_found, invalid_enum_value,
    invalid_parameter, invalid_request, page_size_below_minimum, resource_not_found,
    value_above_maximum_length, value_below_minimum_length, INSTANCE_ID_MAX_LENGTH,
    INSTANCE_ID_NOT_VALID_MESSAGE, INVALID_REQUEST_MESSAGE, QUEUE_NAME_MAX_LENGTH,
    QUEUE_NAME_MIN_LENGTH, QUEUE_NAME_PARAMETER, RESOURCE_NOT_FOUND_MESSAGE,
};

use crate::ops::types::{
    AvailableNumberSummary, ClaimedPhoneNumberSummary, ContactFlow, ContactFlowSummary,
    HoursOfOperation, HoursOfOperationConfig, HoursOfOperationSummary, HoursOfOperationTimeSlice,
    HoursOfOperationsIdentifier, Instance, InstanceStorageConfig, InstanceSummary,
    OutboundCallerConfig, OutboundEmailConfig, PhoneNumberStatus, Queue, QueueSummary,
    PHONE_NUMBER_TYPE_VALUES,
};

/// The region an unsigned request is served in. The emulator's own shared AWS config
/// names `us-east-1` (`vendor/server/src/lib.rs`), so a client that never sets a
/// region still gets ARNs in the region it believes it is talking to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `InstanceStatus` values, from the `com.amazonaws.connect#InstanceStatus` enum.
pub const INSTANCE_STATUS_CREATION_IN_PROGRESS: &str = "CREATION_IN_PROGRESS";
/// See [`INSTANCE_STATUS_CREATION_IN_PROGRESS`].
pub const INSTANCE_STATUS_ACTIVE: &str = "ACTIVE";

/// `QueueStatus` value a freshly created queue carries, from
/// `com.amazonaws.connect#QueueStatus`.
pub const QUEUE_STATUS_ENABLED: &str = "ENABLED";

/// `QueueType` value `CreateQueue` produces, from `com.amazonaws.connect#QueueType`
/// (the other member, `AGENT`, is the implicit per-agent queue, not a created one).
pub const QUEUE_TYPE_STANDARD: &str = "STANDARD";

/// How long a new instance stays in `CREATION_IN_PROGRESS` before the clock owes it
/// `ACTIVE`. Connect returns a new instance in the non-terminal
/// `CREATION_IN_PROGRESS` status, so the transition has to be observable; the span
/// itself is chosen (the emulator does not model the real provisioning time) and is
/// deliberately non-zero so a caller polling straight after the create still sees the
/// transient status.
pub const INSTANCE_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// The default page size for a `List*` that sends no `MaxResults`: the modelled
/// maximum for that operation's page-size member (`MaxResult10` -> 10 for
/// `ListInstances`, `MaxResult1000` -> 1000 for `ListQueues`), so an unpaginated
/// caller sees as much as one page can legally hold.
pub const LIST_INSTANCES_DEFAULT_PAGE_SIZE: usize = 10;
/// See [`LIST_INSTANCES_DEFAULT_PAGE_SIZE`].
pub const LIST_QUEUES_DEFAULT_PAGE_SIZE: usize = 1000;

/// The member name `ListInstances` publishes for its page size, as the model spells it
/// (`smithy_query(query="operation",operation="ListInstances")` -> `MaxResults`): what a
/// refusal of that member names.
pub const MAX_RESULTS_PARAMETER: &str = "MaxResults";

/// The modelled `@range` bounds of `com.amazonaws.connect#MaxResult10`, the shape
/// `ListInstances`'s `MaxResults` targets
/// (`smithy_query(query="operation",operation="ListInstances")` -> range min 1 max 10).
///
/// `ListInstances` applies these two ends DIFFERENTLY, which is why they are named here
/// rather than left to the request boundary: the minimum is a server-side check on the
/// request, and the maximum bounds the page the service builds. The recorded responses
/// are what say so — a call carrying a `maxResults` one past the maximum is answered 200
/// with the page, and one below the minimum is answered 400 with this service's own
/// modelled error. See `crate::ops::list_instances`.
pub const LIST_INSTANCES_MAX_RESULTS_MIN: i32 = 1;
/// See [`LIST_INSTANCES_MAX_RESULTS_MIN`].
pub const LIST_INSTANCES_MAX_RESULTS_MAX: i32 = 10;

/// The page size `ListContactFlows` uses when the caller sends no `MaxResults`: the
/// value its `MaxResults` member documents — "The maximum number of results to return
/// per page. The default MaxResult size is 100."
pub const LIST_CONTACT_FLOWS_DEFAULT_PAGE_SIZE: usize = 100;

/// The page size `ListHoursOfOperations` uses when the caller sends no `MaxResults`:
/// the value its own `MaxResults` member documents — "The maximum number of results to
/// return per page. The default MaxResult size is 100."
pub const LIST_HOURS_OF_OPERATIONS_DEFAULT_PAGE_SIZE: usize = 100;

/// The page size `ListInstanceStorageConfigs` uses when the caller sends no
/// `MaxResults`: its page-size member documents no default
/// (`smithy_query(query="doc",shape="ListInstanceStorageConfigsRequest",member="MaxResults")`
/// -> "The maximum number of results to return per page."), so the modelled maximum of
/// the shape it targets stands in, as [`LIST_INSTANCES_DEFAULT_PAGE_SIZE`] explains
/// (`MaxResult10` -> range 1..10).
pub const LIST_INSTANCE_STORAGE_CONFIGS_DEFAULT_PAGE_SIZE: usize = 10;

/// How many instances the registry holds before `CreateInstance` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// This is the EMULATOR's own bound on its store, not a claim about any AWS
/// account's instance limit (Connect publishes an adjustable per-account quota this
/// emulator does not model): the store lives for the life of the process and the
/// emulator authenticates nobody, so an unbounded create would let any caller that
/// reaches it grow the map until the host runs out of memory. Every record it holds
/// is released by `DeleteInstance`, and it sits far above any workflow an emulator
/// serves. The same bound caps [`ComAmazonawsConnectAmazonconnectserviceState::client_tokens`].
pub const INSTANCE_QUOTA: usize = 1_000;

/// How many retired instance identifiers the store remembers, oldest dropped first.
///
/// The EMULATOR's own bound on its store, for the reason [`INSTANCE_QUOTA`] gives: nothing
/// a caller can do releases a retirement, so without a cap a caller repeating
/// create-and-delete would grow the record until the host ran out of memory. It is the same
/// number as that quota, so the store remembers as many retired identifiers as an account
/// may hold instances at once, and a retirement older than that is reported as an
/// identifier this account never held.
pub const RETIRED_INSTANCE_RECORD: usize = INSTANCE_QUOTA;

/// How many queues one instance holds before `CreateQueue` refuses with the modelled
/// `LimitExceededException`. The emulator's own bound on the store, for the reason
/// [`INSTANCE_QUOTA`] gives; released by `DeleteQueue` (and by deleting the instance).
pub const QUEUES_PER_INSTANCE_QUOTA: usize = 1_000;

/// `ContactFlowStatus` values, from the `com.amazonaws.connect#ContactFlowStatus` enum.
pub const CONTACT_FLOW_STATUS_PUBLISHED: &str = "PUBLISHED";
/// See [`CONTACT_FLOW_STATUS_PUBLISHED`].
pub const CONTACT_FLOW_STATUS_SAVED: &str = "SAVED";

/// `ContactFlowState` value a created flow carries, from
/// `com.amazonaws.connect#ContactFlowState` (the other member, `ARCHIVED`, is what
/// `UpdateContactFlowMetadata` moves a flow to, not a state a create produces).
pub const CONTACT_FLOW_STATE_ACTIVE: &str = "ACTIVE";

/// The alias `DescribeContactFlow` documents for "the saved, not-yet-published
/// content of this flow": `arn:aws:.../contact-flow/{id}:$SAVED`.
pub const CONTACT_FLOW_SAVED_ALIAS: &str = "$SAVED";


/// How many flows one instance holds before `CreateContactFlow` refuses with the
/// modelled `LimitExceededException`. The emulator's own bound on the store, for the
/// reason [`INSTANCE_QUOTA`] gives; released by deleting the instance.
pub const CONTACT_FLOWS_PER_INSTANCE_QUOTA: usize = 1_000;

/// How many hours of operation one instance holds before `CreateHoursOfOperation`
/// refuses with the modelled `LimitExceededException`. The emulator's own bound on the
/// store, for the reason [`INSTANCE_QUOTA`] gives; released by deleting the instance.
pub const HOURS_OF_OPERATIONS_PER_INSTANCE_QUOTA: usize = 1_000;

/// One entry of the default inventory a new instance is provisioned with: a flow's
/// name and the modelled `ContactFlowType` it carries.
#[derive(Debug, Clone, Copy)]
pub struct DefaultContactFlow {
    pub name: &'static str,
    pub flow_type: &'static str,
}

/// The contact flows every new Connect instance is born with.
///
/// AWS stands a new instance up as a complete contact center: it ships with a sample
/// inbound flow plus one default flow for each of the hold/whisper/queue/transfer
/// roles, which is why `ListContactFlows` answers non-empty on an instance nobody has
/// created a flow in (Amazon Connect Administrator Guide, "Sample flows" / "Default
/// flows"). The Smithy model states the operations but not the inventory, so the
/// requirement this encodes is the non-empty default set and the TYPES it covers; the
/// names below are this emulator's rendering of that documented set, not a claim about
/// any particular flow's id. Every `flow_type` is a member of
/// `com.amazonaws.connect#ContactFlowType`.
pub const DEFAULT_CONTACT_FLOWS: [DefaultContactFlow; 9] = [
    DefaultContactFlow {
        name: "Sample inbound flow (first contact experience)",
        flow_type: "CONTACT_FLOW",
    },
    DefaultContactFlow {
        name: "Default agent hold",
        flow_type: "AGENT_HOLD",
    },
    DefaultContactFlow {
        name: "Default agent whisper",
        flow_type: "AGENT_WHISPER",
    },
    DefaultContactFlow {
        name: "Default customer hold",
        flow_type: "CUSTOMER_HOLD",
    },
    DefaultContactFlow {
        name: "Default customer queue",
        flow_type: "CUSTOMER_QUEUE",
    },
    DefaultContactFlow {
        name: "Default customer whisper",
        flow_type: "CUSTOMER_WHISPER",
    },
    DefaultContactFlow {
        name: "Default outbound whisper",
        flow_type: "OUTBOUND_WHISPER",
    },
    DefaultContactFlow {
        name: "Default agent transfer",
        flow_type: "AGENT_TRANSFER",
    },
    DefaultContactFlow {
        name: "Default queue transfer",
        flow_type: "QUEUE_TRANSFER",
    },
];

/// The content each default flow carries: a flow-language document of the form
/// `ContactFlowContent` documents ("The JSON string that represents the content of the
/// flow"), holding the one action that ends the contact. It is the emulator's own
/// default content — AWS publishes no flow document for its samples — and, like any
/// stored content, it is what `DescribeContactFlow` reports and what
/// `FlowContentSha256` is computed from.
pub const DEFAULT_CONTACT_FLOW_CONTENT: &str = concat!(
    r#"{"Version":"2019-10-30","StartAction":"default","Actions":["#,
    r#"{"Identifier":"default","Type":"DisconnectParticipant","Parameters":{}}]}"#
);

/// The names of the queues every new Connect instance is born with: AWS provisions a
/// new instance with one default `STANDARD` queue, documented as `BasicQueue`, so
/// `ListQueues(QueueTypes=["STANDARD"])` answers non-empty on an instance nobody has
/// created a queue in. As with the flows, the requirement is the non-empty default
/// set, not this id.
pub const DEFAULT_QUEUE_NAMES: [&str; 1] = ["BasicQueue"];

/// What the default queue reports as its `Description`.
pub const DEFAULT_QUEUE_DESCRIPTION: &str =
    "The default queue created with the Amazon Connect instance.";

/// The name of the hours of operation every new instance is born with, which the
/// default queue is attached to: AWS provisions a new instance with a default hours of
/// operation documented as `BasicHours`.
pub const DEFAULT_HOURS_OF_OPERATION_NAME: &str = "BasicHours";

/// What the default hours of operation reports as its `Description`.
pub const DEFAULT_HOURS_OF_OPERATION_DESCRIPTION: &str =
    "The default hours of operation created with the Amazon Connect instance.";

/// The `TimeZone` the default hours of operation carries. `com.amazonaws.connect#TimeZone`
/// is an unconstrained string and no AWS document states the default instance's zone
/// (it follows the console's selection, which this emulator does not model), so this is
/// the emulator's own choice of a zone every stored timestamp is already in.
pub const DEFAULT_HOURS_OF_OPERATION_TIME_ZONE: &str = "UTC";

/// The days the default hours of operation covers, from
/// `com.amazonaws.connect#HoursOfOperationDays`.
const DEFAULT_HOURS_OF_OPERATION_DAYS: [&str; 7] = [
    "SUNDAY",
    "MONDAY",
    "TUESDAY",
    "WEDNESDAY",
    "THURSDAY",
    "FRIDAY",
    "SATURDAY",
];

/// Per-service state for com-amazonaws-connect-amazonconnectservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsConnectAmazonconnectserviceState {
    /// Every live instance, keyed by instance id. Queues hang off their instance, so
    /// deleting the instance drops its queues with it. Bounded by [`INSTANCE_QUOTA`].
    pub instances: BTreeMap<String, InstanceRecord>,
    /// The instance identifiers a `DeleteInstance` has retired, oldest first, which is what
    /// tells an identifier this account never held from one it held until a delete dropped
    /// it.
    ///
    /// The two are refused in DIFFERENT words by the operations whose addressed resource
    /// IS the instance (see [`Self::missing_instance_refusal`]), so a delete has to leave
    /// the identifier behind for the later refusal to recognise it. Bounded by
    /// [`RETIRED_INSTANCE_RECORD`]: [`Self::retire_instance`] drops the oldest identifier
    /// once the record is full, so a caller repeating create-and-delete cannot grow it.
    pub retired_instances: VecDeque<String>,
    /// `CreateInstance` idempotency: client token -> the instance it minted. One entry
    /// accompanies one created instance and `DeleteInstance` drops it with that
    /// instance, so this map is bounded by [`INSTANCE_QUOTA`] too — which
    /// `CreateInstance` also checks directly rather than relying on that argument.
    pub client_tokens: BTreeMap<String, String>,
    /// Every phone number claimed to this account, keyed by its phone number id —
    /// the one record `ClaimPhoneNumber` writes, `DescribePhoneNumber` projects and
    /// `ReleasePhoneNumber` removes, and the one answer to "is this inventory number
    /// still available?" (see [`Self::claimed_phone_number`]).
    ///
    /// Bounded without a quota of its own: only a number the emulator's inventory
    /// advertises can be claimed (see [`parse_inventory_number`]), and that inventory
    /// is finite, so this map cannot grow past it.
    pub phone_numbers: BTreeMap<String, PhoneNumberRecord>,
    /// How many identifiers this store has minted. It only ever grows, which is what
    /// makes [`Self::mint_resource_id`] hand out a different id every time.
    pub minted_ids: u64,
}

impl ComAmazonawsConnectAmazonconnectserviceState {
    /// Mint a fresh resource identifier. Connect's instance and queue ids are v4
    /// UUIDs, so this digests the store's monotonic mint counter and lays the 128 bits
    /// out in the `8-4-4-4-12` lowercase-hex form with RFC 4122's version (4) and
    /// variant nibbles set — unique per call, and of the shape the model's `InstanceId`
    /// / `QueueId` carry.
    pub fn mint_resource_id(&mut self) -> String {
        self.minted_ids += 1;
        let digest = Sha256::digest(format!("connect-resource-{}", self.minted_ids));
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex = hex::encode(bytes);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// Record that `instance_id` named an instance of this account until a delete retired
    /// it, dropping the oldest retirement once the record is full
    /// ([`RETIRED_INSTANCE_RECORD`]).
    ///
    /// Idempotent in the identifier: one already recorded keeps its place, so a repeated
    /// delete of the same identifier cannot push older retirements out.
    pub fn retire_instance(&mut self, instance_id: &str) {
        if self.is_retired_instance(instance_id) {
            return;
        }
        self.retired_instances.push_back(instance_id.to_string());
        while self.retired_instances.len() > RETIRED_INSTANCE_RECORD {
            self.retired_instances.pop_front();
        }
    }

    /// Whether `instance_id` named an instance of this account that a delete retired, as
    /// far back as the store remembers ([`RETIRED_INSTANCE_RECORD`]).
    pub fn is_retired_instance(&self, instance_id: &str) -> bool {
        self.retired_instances
            .iter()
            .any(|retired| retired == instance_id)
    }

    /// The instance a phone-number operation's target names, resolved from the
    /// `InstanceId` and `TargetArn` members both `SearchAvailablePhoneNumbers` and
    /// `ClaimPhoneNumber` carry ("You must enter InstanceId or TargetArn").
    ///
    /// A request that names neither is [`TargetProblem::Missing`]; one whose
    /// identifier resolves to no stored instance (an unknown id, an ARN of another
    /// resource, or a traffic distribution group — none can exist until
    /// `CreateTrafficDistributionGroup` stores one) is
    /// [`TargetProblem::Unresolved`]; one that names an instance by id and a
    /// DIFFERENT resource by ARN is [`TargetProblem::Mismatched`], since the two
    /// members name one target.
    pub fn resolve_phone_number_target(
        &self,
        instance_id: Option<&str>,
        target_arn: Option<&str>,
    ) -> Result<&InstanceRecord, TargetProblem> {
        let by_id = instance_id.map(|id| self.instances.get(id));
        let by_arn =
            target_arn.map(|arn| self.instances.values().find(|instance| instance.arn == arn));
        match (by_id, by_arn) {
            (None, None) => Err(TargetProblem::Missing),
            (Some(None), _) | (_, Some(None)) => Err(TargetProblem::Unresolved),
            (Some(Some(by_id)), Some(Some(by_arn))) if by_id.id != by_arn.id => {
                Err(TargetProblem::Mismatched)
            }
            (Some(Some(instance)), _) | (None, Some(Some(instance))) => Ok(instance),
        }
    }

    /// The record of the phone number `phone_number` if this account holds it, or
    /// `None` when the number is not claimed.
    ///
    /// One claim per number: a number this answers for is not available to claim and
    /// is not advertised by `SearchAvailablePhoneNumbers`.
    pub fn claimed_phone_number(&self, phone_number: &str) -> Option<&PhoneNumberRecord> {
        self.phone_numbers
            .values()
            .find(|record| record.phone_number == phone_number)
    }

    /// The record `ClaimPhoneNumber` wrote for `client_token`, or `None` when no
    /// claim carried that token. A repeated call with the token it was first given
    /// reports the claim it made rather than making a second one.
    pub fn phone_number_claimed_with_token(
        &self,
        client_token: &str,
    ) -> Option<&PhoneNumberRecord> {
        self.phone_numbers
            .values()
            .find(|record| record.client_token.as_deref() == Some(client_token))
    }

    /// Every number of the emulator's inventory for `country_code` / `number_type`
    /// that nobody has claimed, in ascending number order, each paired with the
    /// number itself as its pagination key.
    ///
    /// The inventory is derived, not stored: [`inventory_phone_number`] lays out the
    /// [`PHONE_NUMBERS_PER_COUNTRY_AND_TYPE`] numbers of that country and type, and a
    /// number is advertised exactly while no record in [`Self::phone_numbers`] holds
    /// it — so claiming one stops it being advertised and releasing it advertises it
    /// again. `prefix` keeps only the numbers it starts (`PhoneNumberPrefix`,
    /// compared on digits so a prefix sent with or without its leading `+` matches
    /// the same numbers).
    pub fn available_phone_numbers(
        &self,
        country_code: &str,
        number_type: &str,
        prefix: Option<&str>,
    ) -> Vec<(String, AvailableNumberSummary)> {
        let wanted = prefix.map(|value| value.trim_start_matches('+').to_string());
        (0..PHONE_NUMBERS_PER_COUNTRY_AND_TYPE)
            .filter_map(|serial| inventory_phone_number(country_code, number_type, serial))
            .filter(|number| {
                wanted
                    .as_deref()
                    .is_none_or(|digits| number.trim_start_matches('+').starts_with(digits))
            })
            .filter(|number| self.claimed_phone_number(number).is_none())
            .map(|number| {
                let summary = AvailableNumberSummary {
                    phone_number: Some(number.clone()),
                    phone_number_country_code: Some(country_code.to_string()),
                    phone_number_type: Some(number_type.to_string()),
                };
                (number, summary)
            })
            .collect()
    }
}

/// Why a phone-number operation cannot resolve the target its request names. See
/// [`ComAmazonawsConnectAmazonconnectserviceState::resolve_phone_number_target`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetProblem {
    /// The request named neither an `InstanceId` nor a `TargetArn`.
    Missing,
    /// What it named is not a stored instance.
    Unresolved,
    /// Its `InstanceId` and `TargetArn` name different resources.
    Mismatched,
}

impl TargetProblem {
    /// What a refusal of this problem says. One wording for every phone-number
    /// operation that resolves a target, so two of them cannot describe the same
    /// refusal differently.
    pub fn refusal(self) -> &'static str {
        match self {
            Self::Missing => "You must enter InstanceId or TargetArn",
            Self::Unresolved => {
                "The target does not name a Connect instance or traffic distribution group \
                 in this account"
            }
            Self::Mismatched => "InstanceId and TargetArn name different targets",
        }
    }
}

/// `PhoneNumberWorkflowStatus` values, from the
/// `com.amazonaws.connect#PhoneNumberWorkflowStatus` enum: `IN_PROGRESS` while a
/// claim has not completed, `CLAIMED` once it has.
pub const PHONE_NUMBER_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`PHONE_NUMBER_STATUS_IN_PROGRESS`].
pub const PHONE_NUMBER_STATUS_CLAIMED: &str = "CLAIMED";

/// How long after the `ClaimPhoneNumber` call that started it a claim is due to
/// reach `CLAIMED`.
///
/// `ClaimedPhoneNumberSummary$PhoneNumberStatus` documents `IN_PROGRESS` as "a
/// ClaimPhoneNumber ... operation is still in progress and has not yet completed.
/// You can call DescribePhoneNumber at a later time to verify if the previous
/// operation has completed" — so a claimed number is returned in a non-terminal
/// status and the transition has to be observable. AWS documents no duration for it,
/// so the span is the emulator's own and deliberately non-zero: a caller describing
/// the number straight after the claim still sees the transient status. Nothing
/// sleeps for it — the due instant is stored and the status is read off the clock.
pub const PHONE_NUMBER_CLAIM_DELAY: Duration = Duration::from_millis(500);

/// How many numbers of one (country code, phone number type) pair the emulator's
/// claimable inventory holds.
///
/// AWS's available-number pool is account- and carrier-specific and it publishes no
/// size, so this is the emulator's own: more than one page of
/// `SearchAvailablePhoneNumbers`'s largest page (`MaxResult10`, max 10), so
/// pagination is observable, and small enough that the whole inventory of a country
/// and type can be walked.
pub const PHONE_NUMBERS_PER_COUNTRY_AND_TYPE: u32 = 20;

/// The page size `SearchAvailablePhoneNumbers` uses when the caller sends no
/// `MaxResults`: the modelled maximum of its page-size member (`MaxResult10`, range
/// 1..10), so an unpaginated caller sees as much as one page can legally hold.
pub const SEARCH_AVAILABLE_PHONE_NUMBERS_DEFAULT_PAGE_SIZE: usize = 10;

/// The E.164 country calling code of every ISO country
/// `com.amazonaws.connect#PhoneNumberCountryCode` names, keyed by its ISO 3166-1
/// alpha-2 code and sorted by it (see [`country_calling_code`]).
///
/// Source: the ITU-T Recommendation E.164 assigned country-code list (ITU-T
/// Operational Bulletin "List of ITU-T Recommendation E.164 assigned country
/// codes"), which is what the leading digits of a number in that country are. The
/// catalog is the model's own: `PHONE_NUMBER_COUNTRY_CODE_VALUES` and this table
/// cover exactly the same countries, which this crate's tests assert.
pub const COUNTRY_CALLING_CODES: &[(&str, &str)] = &[
    ("AD", "376"),
    ("AE", "971"),
    ("AF", "93"),
    ("AG", "1"),
    ("AI", "1"),
    ("AL", "355"),
    ("AM", "374"),
    ("AN", "599"),
    ("AO", "244"),
    ("AQ", "672"),
    ("AR", "54"),
    ("AS", "1"),
    ("AT", "43"),
    ("AU", "61"),
    ("AW", "297"),
    ("AZ", "994"),
    ("BA", "387"),
    ("BB", "1"),
    ("BD", "880"),
    ("BE", "32"),
    ("BF", "226"),
    ("BG", "359"),
    ("BH", "973"),
    ("BI", "257"),
    ("BJ", "229"),
    ("BL", "590"),
    ("BM", "1"),
    ("BN", "673"),
    ("BO", "591"),
    ("BR", "55"),
    ("BS", "1"),
    ("BT", "975"),
    ("BW", "267"),
    ("BY", "375"),
    ("BZ", "501"),
    ("CA", "1"),
    ("CC", "61"),
    ("CD", "243"),
    ("CF", "236"),
    ("CG", "242"),
    ("CH", "41"),
    ("CI", "225"),
    ("CK", "682"),
    ("CL", "56"),
    ("CM", "237"),
    ("CN", "86"),
    ("CO", "57"),
    ("CR", "506"),
    ("CU", "53"),
    ("CV", "238"),
    ("CW", "599"),
    ("CX", "61"),
    ("CY", "357"),
    ("CZ", "420"),
    ("DE", "49"),
    ("DJ", "253"),
    ("DK", "45"),
    ("DM", "1"),
    ("DO", "1"),
    ("DZ", "213"),
    ("EC", "593"),
    ("EE", "372"),
    ("EG", "20"),
    ("EH", "212"),
    ("ER", "291"),
    ("ES", "34"),
    ("ET", "251"),
    ("FI", "358"),
    ("FJ", "679"),
    ("FK", "500"),
    ("FM", "691"),
    ("FO", "298"),
    ("FR", "33"),
    ("GA", "241"),
    ("GB", "44"),
    ("GD", "1"),
    ("GE", "995"),
    ("GG", "44"),
    ("GH", "233"),
    ("GI", "350"),
    ("GL", "299"),
    ("GM", "220"),
    ("GN", "224"),
    ("GQ", "240"),
    ("GR", "30"),
    ("GT", "502"),
    ("GU", "1"),
    ("GW", "245"),
    ("GY", "592"),
    ("HK", "852"),
    ("HN", "504"),
    ("HR", "385"),
    ("HT", "509"),
    ("HU", "36"),
    ("ID", "62"),
    ("IE", "353"),
    ("IL", "972"),
    ("IM", "44"),
    ("IN", "91"),
    ("IO", "246"),
    ("IQ", "964"),
    ("IR", "98"),
    ("IS", "354"),
    ("IT", "39"),
    ("JE", "44"),
    ("JM", "1"),
    ("JO", "962"),
    ("JP", "81"),
    ("KE", "254"),
    ("KG", "996"),
    ("KH", "855"),
    ("KI", "686"),
    ("KM", "269"),
    ("KN", "1"),
    ("KP", "850"),
    ("KR", "82"),
    ("KW", "965"),
    ("KY", "1"),
    ("KZ", "7"),
    ("LA", "856"),
    ("LB", "961"),
    ("LC", "1"),
    ("LI", "423"),
    ("LK", "94"),
    ("LR", "231"),
    ("LS", "266"),
    ("LT", "370"),
    ("LU", "352"),
    ("LV", "371"),
    ("LY", "218"),
    ("MA", "212"),
    ("MC", "377"),
    ("MD", "373"),
    ("ME", "382"),
    ("MF", "590"),
    ("MG", "261"),
    ("MH", "692"),
    ("MK", "389"),
    ("ML", "223"),
    ("MM", "95"),
    ("MN", "976"),
    ("MO", "853"),
    ("MP", "1"),
    ("MR", "222"),
    ("MS", "1"),
    ("MT", "356"),
    ("MU", "230"),
    ("MV", "960"),
    ("MW", "265"),
    ("MX", "52"),
    ("MY", "60"),
    ("MZ", "258"),
    ("NA", "264"),
    ("NC", "687"),
    ("NE", "227"),
    ("NG", "234"),
    ("NI", "505"),
    ("NL", "31"),
    ("NO", "47"),
    ("NP", "977"),
    ("NR", "674"),
    ("NU", "683"),
    ("NZ", "64"),
    ("OM", "968"),
    ("PA", "507"),
    ("PE", "51"),
    ("PF", "689"),
    ("PG", "675"),
    ("PH", "63"),
    ("PK", "92"),
    ("PL", "48"),
    ("PM", "508"),
    ("PN", "64"),
    ("PR", "1"),
    ("PT", "351"),
    ("PW", "680"),
    ("PY", "595"),
    ("QA", "974"),
    ("RE", "262"),
    ("RO", "40"),
    ("RS", "381"),
    ("RU", "7"),
    ("RW", "250"),
    ("SA", "966"),
    ("SB", "677"),
    ("SC", "248"),
    ("SD", "249"),
    ("SE", "46"),
    ("SG", "65"),
    ("SH", "290"),
    ("SI", "386"),
    ("SJ", "47"),
    ("SK", "421"),
    ("SL", "232"),
    ("SM", "378"),
    ("SN", "221"),
    ("SO", "252"),
    ("SR", "597"),
    ("ST", "239"),
    ("SV", "503"),
    ("SX", "1"),
    ("SY", "963"),
    ("SZ", "268"),
    ("TC", "1"),
    ("TD", "235"),
    ("TG", "228"),
    ("TH", "66"),
    ("TJ", "992"),
    ("TK", "690"),
    ("TL", "670"),
    ("TM", "993"),
    ("TN", "216"),
    ("TO", "676"),
    ("TR", "90"),
    ("TT", "1"),
    ("TV", "688"),
    ("TW", "886"),
    ("TZ", "255"),
    ("UA", "380"),
    ("UG", "256"),
    ("US", "1"),
    ("UY", "598"),
    ("UZ", "998"),
    ("VA", "39"),
    ("VC", "1"),
    ("VE", "58"),
    ("VG", "1"),
    ("VI", "1"),
    ("VN", "84"),
    ("VU", "678"),
    ("WF", "681"),
    ("WS", "685"),
    ("YE", "967"),
    ("YT", "262"),
    ("ZA", "27"),
    ("ZM", "260"),
    ("ZW", "263"),
];

/// The E.164 country calling code of the ISO country `country_code`, or `None` when
/// [`COUNTRY_CALLING_CODES`] does not name it.
pub fn country_calling_code(country_code: &str) -> Option<&'static str> {
    COUNTRY_CALLING_CODES
        .binary_search_by(|(iso, _)| (*iso).cmp(country_code))
        .ok()
        .map(|found| COUNTRY_CALLING_CODES[found].1)
}

/// The slot an ISO 3166-1 alpha-2 code occupies in the 26x26 grid of two-letter
/// codes: `0..=675`, different for every country, so the inventory numbers of two
/// countries that share one calling code (`US` and `CA` share `1`) are still
/// different numbers.
fn country_slot(country_code: &str) -> Option<u32> {
    let mut letters = country_code.bytes();
    let (Some(first), Some(second), None) = (letters.next(), letters.next(), letters.next()) else {
        return None;
    };
    if !first.is_ascii_uppercase() || !second.is_ascii_uppercase() {
        return None;
    }
    Some(u32::from(first - b'A') * 26 + u32::from(second - b'A'))
}

/// The ISO country whose [`country_slot`] is `slot`, or `None` when `slot` is past
/// the grid.
fn country_of_slot(slot: u32) -> Option<String> {
    if slot >= 26 * 26 {
        return None;
    }
    let first = b'A' + (slot / 26) as u8;
    let second = b'A' + (slot % 26) as u8;
    Some(String::from_utf8(vec![first, second]).expect("two ASCII letters"))
}

/// The `serial`th number of the emulator's claimable inventory for `country_code`
/// and `number_type`, or `None` when the country is not in
/// [`COUNTRY_CALLING_CODES`], the type is not in the model's `PhoneNumberType`
/// catalog, or the serial is past [`PHONE_NUMBERS_PER_COUNTRY_AND_TYPE`].
///
/// The number is `+<calling code><country slot, 3 digits><type slot><serial, 4
/// digits>`: E.164, as the modelled `PhoneNumber` (`^\+[1-9]\d{1,14}$`) requires —
/// at most 3 + 3 + 1 + 4 = 11 digits — carrying the calling code of the country it
/// is offered for, and unique across every (country, type, serial) triple, so no two
/// inventories advertise the same number. It is a pure function of those three, so
/// the inventory does not have to be stored and every handler agrees on it.
pub fn inventory_phone_number(
    country_code: &str,
    number_type: &str,
    serial: u32,
) -> Option<String> {
    if serial >= PHONE_NUMBERS_PER_COUNTRY_AND_TYPE {
        return None;
    }
    let calling_code = country_calling_code(country_code)?;
    let slot = country_slot(country_code)?;
    let type_slot = PHONE_NUMBER_TYPE_VALUES
        .iter()
        .position(|value| *value == number_type)?;
    Some(format!("+{calling_code}{slot:03}{type_slot}{serial:04}"))
}

/// Which inventory number `phone_number` is, or `None` when it is not one at all —
/// which is what makes a number claimable: `ClaimPhoneNumber` takes only a number
/// `SearchAvailablePhoneNumbers` advertises.
///
/// The exact inverse of [`inventory_phone_number`]: the last eight digits are the
/// country slot, the type slot and the serial, and what precedes them has to be the
/// calling code of the country that slot names.
pub fn parse_inventory_number(phone_number: &str) -> Option<InventoryNumber> {
    let digits = phone_number.strip_prefix('+')?;
    if !digits.bytes().all(|digit| digit.is_ascii_digit()) || digits.len() < 9 {
        return None;
    }
    let (calling_code, tail) = digits.split_at(digits.len() - 8);
    let country_code = country_of_slot(tail[..3].parse().ok()?)?;
    if country_calling_code(&country_code) != Some(calling_code) {
        return None;
    }
    let type_slot: usize = tail[3..4].parse().ok()?;
    let number_type = PHONE_NUMBER_TYPE_VALUES.get(type_slot)?;
    let serial: u32 = tail[4..].parse().ok()?;
    if serial >= PHONE_NUMBERS_PER_COUNTRY_AND_TYPE {
        return None;
    }
    Some(InventoryNumber {
        country_code,
        number_type: (*number_type).to_string(),
        serial,
    })
}

/// The inventory entry one E.164 number is, as [`parse_inventory_number`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryNumber {
    /// The ISO country the number is offered for.
    pub country_code: String,
    /// The `PhoneNumberType` it is offered as.
    pub number_type: String,
    /// Its position in that country and type's inventory.
    pub serial: u32,
}

/// `arn:aws:connect:<region>:<account>:phone-number/<id>`: the ARN form Connect's own
/// IAM resource types document for a claimed phone number, which is account-scoped
/// rather than instance-scoped (a number can be re-targeted between instances).
pub fn phone_number_arn(region: &str, phone_number_id: &str) -> String {
    ::engine::emulator_arn("connect", region, &format!("phone-number/{phone_number_id}"))
}

/// One phone number claimed to this account.
#[derive(Debug, Clone)]
pub struct PhoneNumberRecord {
    pub id: String,
    pub arn: String,
    /// The E.164 number, exactly as the inventory advertised it.
    pub phone_number: String,
    /// The ISO country and the `PhoneNumberType` of the inventory entry this number
    /// came from — read off the number itself (see [`parse_inventory_number`]), so
    /// what `DescribePhoneNumber` reports cannot disagree with what
    /// `SearchAvailablePhoneNumbers` advertised.
    pub country_code: String,
    /// See [`PhoneNumberRecord::country_code`].
    pub number_type: String,
    pub description: Option<String>,
    /// The instance the number's inbound traffic is routed through, and its ARN:
    /// the target the claim resolved, never the text the request carried.
    pub instance_id: String,
    /// See [`PhoneNumberRecord::instance_id`].
    pub target_arn: String,
    pub tags: BTreeMap<String, String>,
    /// The `ClientToken` the claim carried, which is what makes a repeated claim
    /// report the first one rather than make a second (`@idempotencyToken`).
    pub client_token: Option<String>,
    /// The instant this claim reaches `CLAIMED`: the claim instant plus
    /// [`PHONE_NUMBER_CLAIM_DELAY`]. Before it the number reports `IN_PROGRESS`.
    pub claimed_at: SystemTime,
}

impl PhoneNumberRecord {
    /// The claim's workflow status at `now` — derived from the stored instant, so
    /// reading it changes nothing a later caller sees.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.claimed_at + PHONE_NUMBER_CLAIM_DELAY {
            PHONE_NUMBER_STATUS_IN_PROGRESS
        } else {
            PHONE_NUMBER_STATUS_CLAIMED
        }
    }

    /// The modelled `ClaimedPhoneNumberSummary` shape `DescribePhoneNumber` returns.
    ///
    /// `SourcePhoneNumberArn` stays absent: it is the ARN a number IMPORTED from
    /// another service carries, and `ImportPhoneNumber` is not implemented, so no
    /// stored number has one.
    pub fn to_summary(&self, now: SystemTime) -> ClaimedPhoneNumberSummary {
        ClaimedPhoneNumberSummary {
            phone_number_id: Some(self.id.clone()),
            phone_number_arn: Some(self.arn.clone()),
            phone_number: Some(self.phone_number.clone()),
            phone_number_country_code: Some(self.country_code.clone()),
            phone_number_type: Some(self.number_type.clone()),
            phone_number_description: self.description.clone(),
            target_arn: Some(self.target_arn.clone()),
            instance_id: Some(self.instance_id.clone()),
            tags: tags_out(&self.tags),
            phone_number_status: Some(PhoneNumberStatus {
                status: Some(self.status(now).to_string()),
                message: None,
            }),
            source_phone_number_arn: None,
        }
    }
}

/// One stored Connect instance.
///
/// Built through [`InstanceRecord::new`], which is also the only way a crate other than
/// this one can build one: the record carries the resources that hang off an instance,
/// so it grows a member whenever another of them is stored, and a member added here is
/// given its value for every instance in that one constructor instead of in every
/// caller.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct InstanceRecord {
    pub id: String,
    pub arn: String,
    pub region: String,
    pub identity_management_type: String,
    pub instance_alias: Option<String>,
    pub directory_id: Option<String>,
    pub inbound_calls_enabled: bool,
    pub outbound_calls_enabled: bool,
    pub service_role: String,
    pub instance_access_url: String,
    pub created_time: SystemTime,
    /// The instant the clock owes this instance `ACTIVE`; see
    /// [`INSTANCE_ACTIVATION_DELAY`].
    pub active_at: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// This instance's queues, keyed by queue id.
    pub queues: BTreeMap<String, QueueRecord>,
    /// This instance's flows, keyed by flow id. Bounded by
    /// [`CONTACT_FLOWS_PER_INSTANCE_QUOTA`].
    pub contact_flows: BTreeMap<String, ContactFlowRecord>,
    /// This instance's hours of operation, keyed by hours-of-operation id. The queues
    /// of the instance name one of these through their `HoursOfOperationId`, which is
    /// why the default queue's hours live here rather than beside it. Bounded by
    /// [`HOURS_OF_OPERATIONS_PER_INSTANCE_QUOTA`].
    pub hours_of_operations: BTreeMap<String, HoursOfOperationRecord>,
    /// The chats `StartChatContact` started in this instance, keyed by contact id.
    /// Bounded by [`CHAT_CONTACTS_PER_INSTANCE_QUOTA`].
    pub chat_contacts: BTreeMap<String, ChatContactRecord>,
    /// This instance's storage configurations, keyed by association id — the one
    /// record `AssociateInstanceStorageConfig` writes and
    /// `DescribeInstanceStorageConfig` projects.
    ///
    /// Bounded without a quota of its own: an instance holds at most one
    /// configuration per `InstanceStorageResourceType` (see
    /// [`Self::storage_config_for_resource_type`]) and that enum is finite, so this
    /// map cannot grow past it.
    pub storage_configs: BTreeMap<String, StorageConfigRecord>,
    /// This instance's attached-files configurations, keyed by `AttachmentScope` —
    /// the records `UpdateAttachedFilesConfiguration` writes and
    /// `DescribeAttachedFilesConfiguration` projects. A scope nobody has updated holds
    /// no record and reports its defaults (see
    /// [`Self::attached_files_configuration`]).
    ///
    /// Bounded without a quota of its own: one record per `AttachmentScope`, and that
    /// enum is finite.
    pub attached_files_configurations: BTreeMap<String, AttachedFilesConfigurationRecord>,
    /// The feature toggles `UpdateInstanceAttribute` has changed on this instance, keyed
    /// by `InstanceAttributeType`. A type nobody has updated holds no entry and reports
    /// its default, and the two call directions are never keyed here because they are the
    /// instance's own flags (see [`Self::instance_attribute_value`]).
    ///
    /// Bounded without a quota of its own: the request boundary admits only the values
    /// `InstanceAttributeType` declares, and that enum is finite.
    pub instance_attributes: BTreeMap<String, String>,
}

impl InstanceRecord {
    /// The record a created instance starts from: instance `id` in `region`, with
    /// `identity_management_type` (one of the modelled `DirectoryType` values) and
    /// `instance_alias`, created at `created_time` and owed `ACTIVE` at `active_at`.
    ///
    /// Everything else is derived or empty: the ARN, the service-linked role and the
    /// access URL follow from the id and the alias, no directory is named, both call
    /// directions are enabled, and the instance holds no tags and none of the resources
    /// that hang off it — a caller that wants other values assigns those members on the
    /// record this hands back.
    pub fn new(
        region: &str,
        id: &str,
        identity_management_type: &str,
        instance_alias: Option<String>,
        created_time: SystemTime,
        active_at: SystemTime,
    ) -> Self {
        Self {
            id: id.to_string(),
            arn: instance_arn(region, id),
            region: region.to_string(),
            identity_management_type: identity_management_type.to_string(),
            instance_access_url: instance_access_url(instance_alias.as_deref(), id),
            instance_alias,
            directory_id: None,
            inbound_calls_enabled: true,
            outbound_calls_enabled: true,
            service_role: instance_service_role(id),
            created_time,
            active_at,
            tags: BTreeMap::new(),
            queues: BTreeMap::new(),
            contact_flows: BTreeMap::new(),
            hours_of_operations: BTreeMap::new(),
            chat_contacts: BTreeMap::new(),
            storage_configs: BTreeMap::new(),
            attached_files_configurations: BTreeMap::new(),
            instance_attributes: BTreeMap::new(),
        }
    }

    /// The instance's status at `now` — derived from the stored activation instant, so
    /// reading it changes nothing a later caller sees.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.active_at {
            INSTANCE_STATUS_CREATION_IN_PROGRESS
        } else {
            INSTANCE_STATUS_ACTIVE
        }
    }

    /// The modelled `Instance` shape `DescribeInstance` returns.
    pub fn to_instance(&self, now: SystemTime) -> Instance {
        Instance {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            identity_management_type: Some(self.identity_management_type.clone()),
            instance_alias: self.instance_alias.clone(),
            created_time: Some(to_date_time(self.created_time)),
            service_role: Some(self.service_role.clone()),
            instance_status: Some(self.status(now).to_string()),
            status_reason: None,
            inbound_calls_enabled: Some(self.inbound_calls_enabled),
            outbound_calls_enabled: Some(self.outbound_calls_enabled),
            instance_access_url: Some(self.instance_access_url.clone()),
            tags: tags_out(&self.tags),
        }
    }

    /// The modelled `InstanceSummary` shape `ListInstances` returns.
    pub fn to_summary(&self, now: SystemTime) -> InstanceSummary {
        InstanceSummary {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            identity_management_type: Some(self.identity_management_type.clone()),
            instance_alias: self.instance_alias.clone(),
            created_time: Some(to_date_time(self.created_time)),
            service_role: Some(self.service_role.clone()),
            instance_status: Some(self.status(now).to_string()),
            inbound_calls_enabled: Some(self.inbound_calls_enabled),
            outbound_calls_enabled: Some(self.outbound_calls_enabled),
            instance_access_url: Some(self.instance_access_url.clone()),
        }
    }

    /// The id of this instance's queue called `name`, if it has one. Queue names are
    /// unique within an instance, which is what makes a same-name create a duplicate.
    pub fn queue_id_named(&self, name: &str) -> Option<&str> {
        self.queues
            .values()
            .find(|queue| queue.name == name)
            .map(|queue| queue.id.as_str())
    }

    /// The id of this instance's flow called `name`, if it has one. Flow names are
    /// unique within an instance, which is what makes a same-name create a duplicate.
    pub fn contact_flow_id_named(&self, name: &str) -> Option<&str> {
        self.contact_flows
            .values()
            .find(|flow| flow.name == name)
            .map(|flow| flow.id.as_str())
    }

    /// The id of this instance's hours of operation called `name`, if it has one. An
    /// hours-of-operation name is unique within an instance, which is what makes a
    /// same-name create a duplicate.
    pub fn hours_of_operation_id_named(&self, name: &str) -> Option<&str> {
        self.hours_of_operations
            .values()
            .find(|hours| hours.name == name)
            .map(|hours| hours.id.as_str())
    }

    /// The modelled `HoursOfOperation` shape `DescribeHoursOfOperation` returns, with
    /// every stored parent reference resolved against this instance's own records (the
    /// modelled `HoursOfOperationsIdentifier` carries the parent's name and ARN beside
    /// its id, and both live on the parent's record rather than on the child's).
    pub fn hours_of_operation_detail(&self, id: &str) -> Option<HoursOfOperation> {
        let record = self.hours_of_operations.get(id)?;
        let parents: Vec<HoursOfOperationsIdentifier> = record
            .parent_ids
            .iter()
            .filter_map(|parent_id| self.hours_of_operations.get(parent_id))
            .map(|parent| HoursOfOperationsIdentifier {
                name: parent.name.clone(),
                id: parent.id.clone(),
                arn: Some(parent.arn.clone()),
            })
            .collect();
        Some(record.to_hours_of_operation(parents))
    }

    /// This instance's flow named by a `ContactFlowId` path label.
    ///
    /// `DescribeContactFlow` documents three forms for that label: the bare flow id,
    /// the flow's ARN, and either of those with a `:$SAVED` (or `:<version>`) suffix.
    /// The emulator stores one content per flow — the saved content, which publishing
    /// does not change — so the `$SAVED` alias resolves to the same record, while a
    /// numbered version resolves to nothing until flow versions are stored.
    pub fn contact_flow_for_label(&self, label: &str) -> Option<&ContactFlowRecord> {
        // An ARN's own colons all precede its resource segment, so taking the last
        // `/`-separated segment first leaves at most the `:<qualifier>` suffix.
        let resource = label.rsplit_once('/').map_or(label, |(_, last)| last);
        let (id, qualifier) = match resource.split_once(':') {
            Some((id, qualifier)) => (id, Some(qualifier)),
            None => (resource, None),
        };
        if qualifier.is_some_and(|qualifier| qualifier != CONTACT_FLOW_SAVED_ALIAS) {
            return None;
        }
        self.contact_flows.get(id)
    }

    /// This instance's storage configuration for `resource_type`, if it has one.
    ///
    /// One configuration per resource type is what makes a second association of the
    /// same resource type a conflict rather than a second configuration.
    pub fn storage_config_for_resource_type(
        &self,
        resource_type: &str,
    ) -> Option<&StorageConfigRecord> {
        self.storage_configs
            .values()
            .find(|record| record.resource_type == resource_type)
    }

    /// This instance's storage configuration `association_id` names, but only when it
    /// stores `resource_type`.
    ///
    /// An association identifies one resource type's configuration, so an association
    /// id paired with another resource type names nothing.
    pub fn storage_config(
        &self,
        association_id: &str,
        resource_type: &str,
    ) -> Option<&StorageConfigRecord> {
        self.storage_configs
            .get(association_id)
            .filter(|record| record.resource_type == resource_type)
    }

    /// The same pairing [`Self::storage_config`] resolves, for writing: what
    /// `UpdateInstanceStorageConfig` replaces the configuration of.
    pub fn storage_config_mut(
        &mut self,
        association_id: &str,
        resource_type: &str,
    ) -> Option<&mut StorageConfigRecord> {
        self.storage_configs
            .get_mut(association_id)
            .filter(|record| record.resource_type == resource_type)
    }

    /// The storage configuration this instance's association with `client_token`
    /// produced for `resource_type`, if that token already made one.
    pub fn storage_config_associated_with_token(
        &self,
        client_token: &str,
        resource_type: &str,
    ) -> Option<&StorageConfigRecord> {
        self.storage_configs.values().find(|record| {
            record.client_token.as_deref() == Some(client_token)
                && record.resource_type == resource_type
        })
    }
}

/// One storage configuration associated with an instance.
#[derive(Debug, Clone)]
pub struct StorageConfigRecord {
    /// The association identifier the association minted, which is what
    /// `DescribeInstanceStorageConfig` and `DisassociateInstanceStorageConfig` name
    /// this configuration by.
    pub association_id: String,
    /// The `InstanceStorageResourceType` whose data this configuration stores.
    pub resource_type: String,
    /// The configuration as associated, carrying [`Self::association_id`] in its own
    /// `AssociationId` member.
    pub config: InstanceStorageConfig,
    /// The `ClientToken` the association carried, when it carried one — what makes a
    /// retry of that same association report it rather than make a second one.
    pub client_token: Option<String>,
}

impl StorageConfigRecord {
    /// The modelled `InstanceStorageConfig` shape `DescribeInstanceStorageConfig`
    /// returns and `ListInstanceStorageConfigs` lists.
    pub fn to_storage_config(&self) -> InstanceStorageConfig {
        let mut config = self.config.clone();
        config.association_id = Some(self.association_id.clone());
        config
    }

    /// Whether `config` describes the same storage resource this record already
    /// holds: every `InstanceStorageConfig` member but the association id, which the
    /// association mints rather than the caller.
    pub fn describes(&self, config: &InstanceStorageConfig) -> bool {
        let mut candidate = config.clone();
        candidate.association_id = Some(self.association_id.clone());
        candidate == self.config
    }

    /// Replace the configuration this record holds with `config`.
    ///
    /// An update changes WHERE a resource type's data is stored, never which
    /// association stores it: the association keeps its own identifier and resource
    /// type, and the stored configuration carries that identifier whatever
    /// `AssociationId` the request's own configuration held.
    pub fn apply(&mut self, mut config: InstanceStorageConfig) {
        config.association_id = Some(self.association_id.clone());
        self.config = config;
    }
}

/// Which `InstanceStorageConfig` member each `StorageType` names, from
/// `com.amazonaws.connect#StorageType` and the members of
/// `com.amazonaws.connect#InstanceStorageConfig`: the storage type says which one of
/// the four configuration members carries that storage resource's settings.
///
/// Each entry carries the member's name and the predicate that reads it, so an entry
/// added for a new storage type cannot be read against the wrong member.
pub type StorageTypeConfigMember = (&'static str, &'static str, fn(&InstanceStorageConfig) -> bool);

/// See [`StorageTypeConfigMember`].
pub const STORAGE_TYPE_CONFIG_MEMBERS: [StorageTypeConfigMember; 4] = [
    ("S3", "S3Config", |config| config.s3_config.is_some()),
    (
        "KINESIS_VIDEO_STREAM",
        "KinesisVideoStreamConfig",
        |config| config.kinesis_video_stream_config.is_some(),
    ),
    ("KINESIS_STREAM", "KinesisStreamConfig", |config| {
        config.kinesis_stream_config.is_some()
    }),
    ("KINESIS_FIREHOSE", "KinesisFirehoseConfig", |config| {
        config.kinesis_firehose_config.is_some()
    }),
];

/// Why `config` is not a usable storage configuration, or `None` when it is.
///
/// A storage configuration carries the settings of the storage type it names, so the
/// member [`STORAGE_TYPE_CONFIG_MEMBERS`] pairs with its `StorageType` has to be
/// there; a configuration without it names a storage resource it does not describe.
pub fn storage_config_problem(config: &InstanceStorageConfig) -> Option<String> {
    let Some((_, member, carried)) = STORAGE_TYPE_CONFIG_MEMBERS
        .iter()
        .copied()
        .find(|(storage_type, _, _)| *storage_type == config.storage_type)
    else {
        return Some(format!(
            "StorageType {} is not a supported storage type",
            config.storage_type
        ));
    };
    if carried(config) {
        None
    } else {
        Some(format!(
            "StorageConfig for StorageType {} must carry {member}",
            config.storage_type
        ))
    }
}

/// One stored queue, inside its instance.
#[derive(Debug, Clone)]
pub struct QueueRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub hours_of_operation_id: String,
    pub max_contacts: Option<i32>,
    pub outbound_caller_config: Option<OutboundCallerConfig>,
    pub outbound_email_config: Option<OutboundEmailConfig>,
    /// Kept as given: no modelled `Queue`/`QueueSummary` member carries it, and the
    /// operations that read it (`ListQueueQuickConnects`,
    /// `AssociateQueueQuickConnects`) are still scaffolded.
    pub quick_connect_ids: Vec<String>,
    pub queue_type: String,
    pub status: String,
    pub tags: BTreeMap<String, String>,
    pub last_modified_time: SystemTime,
    pub last_modified_region: String,
}

impl QueueRecord {
    /// The modelled `Queue` shape `DescribeQueue` returns.
    pub fn to_queue(&self) -> Queue {
        Queue {
            name: Some(self.name.clone()),
            queue_arn: Some(self.arn.clone()),
            queue_id: Some(self.id.clone()),
            description: self.description.clone(),
            outbound_caller_config: self.outbound_caller_config.clone(),
            outbound_email_config: self.outbound_email_config.clone(),
            hours_of_operation_id: Some(self.hours_of_operation_id.clone()),
            max_contacts: self.max_contacts,
            status: Some(self.status.clone()),
            tags: tags_out(&self.tags),
            last_modified_time: Some(to_date_time(self.last_modified_time)),
            last_modified_region: Some(self.last_modified_region.clone()),
        }
    }

    /// The modelled `QueueSummary` shape `ListQueues` returns.
    pub fn to_summary(&self) -> QueueSummary {
        QueueSummary {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            queue_type: Some(self.queue_type.clone()),
            last_modified_time: Some(to_date_time(self.last_modified_time)),
            last_modified_region: Some(self.last_modified_region.clone()),
        }
    }
}

/// One stored hours of operation, inside its instance.
#[derive(Debug, Clone)]
pub struct HoursOfOperationRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// The time zone as given, kept verbatim: the modelled `TimeZone` is an
    /// unconstrained string, so the store is what any later read reports.
    pub time_zone: String,
    /// The modelled `HoursOfOperationConfigList`: one entry per covered day.
    pub config: Vec<HoursOfOperationConfig>,
    /// The ids named by `ParentHoursOfOperationConfigs`, in the order given. The name
    /// and ARN a read reports for each one are read off the parent's own record, so
    /// renaming a parent cannot leave a stale copy behind.
    pub parent_ids: Vec<String>,
    pub tags: BTreeMap<String, String>,
    pub last_modified_time: SystemTime,
    pub last_modified_region: String,
}

/// One stored flow, inside its instance.
#[derive(Debug, Clone)]
pub struct ContactFlowRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// The modelled `ContactFlowType` the flow was created with.
    pub flow_type: String,
    /// The modelled `ContactFlowState`; see [`CONTACT_FLOW_STATE_ACTIVE`].
    pub state: String,
    /// The modelled `ContactFlowStatus`; see [`CONTACT_FLOW_STATUS_PUBLISHED`].
    pub status: String,
    /// The flow language document the caller sent, kept verbatim.
    pub content: String,
    pub tags: BTreeMap<String, String>,
    pub last_modified_time: SystemTime,
    pub last_modified_region: String,
}

impl ContactFlowRecord {
    /// The modelled `ContactFlowSummary` shape `ListContactFlows` returns.
    pub fn to_summary(&self) -> ContactFlowSummary {
        ContactFlowSummary {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            contact_flow_type: Some(self.flow_type.clone()),
            contact_flow_state: Some(self.state.clone()),
            contact_flow_status: Some(self.status.clone()),
        }
    }

    /// The modelled `ContactFlow` shape `DescribeContactFlow` returns.
    ///
    /// `Version`/`VersionDescription` stay absent: a flow version is only minted by
    /// `CreateContactFlowVersion`, which this emulator does not implement yet, so there
    /// is no stored version to report.
    pub fn to_contact_flow(&self) -> ContactFlow {
        ContactFlow {
            arn: Some(self.arn.clone()),
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            r#type: Some(self.flow_type.clone()),
            state: Some(self.state.clone()),
            status: Some(self.status.clone()),
            description: self.description.clone(),
            content: Some(self.content.clone()),
            tags: tags_out(&self.tags),
            flow_content_sha256: Some(flow_content_sha256(&self.content)),
            version: None,
            version_description: None,
            last_modified_time: Some(to_date_time(self.last_modified_time)),
            last_modified_region: Some(self.last_modified_region.clone()),
        }
    }
}

impl HoursOfOperationRecord {
    /// The modelled `HoursOfOperationSummary` shape `ListHoursOfOperations` returns.
    pub fn to_summary(&self) -> HoursOfOperationSummary {
        HoursOfOperationSummary {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            last_modified_time: Some(to_date_time(self.last_modified_time)),
            last_modified_region: Some(self.last_modified_region.clone()),
        }
    }

    /// The modelled `HoursOfOperation` shape `DescribeHoursOfOperation` returns.
    /// `parents` is this record's parent references already resolved; see
    /// [`InstanceRecord::hours_of_operation_detail`], which is how a handler reaches it.
    pub fn to_hours_of_operation(
        &self,
        parents: Vec<HoursOfOperationsIdentifier>,
    ) -> HoursOfOperation {
        HoursOfOperation {
            hours_of_operation_id: Some(self.id.clone()),
            hours_of_operation_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            time_zone: Some(self.time_zone.clone()),
            config: Some(self.config.clone()),
            // An hours of operation with no parent reports no `ParentHoursOfOperations`
            // member at all rather than an empty list.
            parent_hours_of_operations: if parents.is_empty() {
                None
            } else {
                Some(parents)
            },
            tags: tags_out(&self.tags),
            last_modified_time: Some(to_date_time(self.last_modified_time)),
            last_modified_region: Some(self.last_modified_region.clone()),
        }
    }
}

/// `<instance ARN>/operating-hours/<id>`: the hours-of-operation ARN form Connect's own
/// IAM resource types document, so it is derived from the instance that holds it.
pub fn hours_of_operation_arn(instance_arn: &str, hours_of_operation_id: &str) -> String {
    format!("{instance_arn}/operating-hours/{hours_of_operation_id}")
}

/// Provision the resources a new instance is born with, into the instance `instance_id`
/// of `store`: the default contact flows ([`DEFAULT_CONTACT_FLOWS`]), the default
/// `STANDARD` queue(s) ([`DEFAULT_QUEUE_NAMES`]) and the default hours of operation
/// ([`DEFAULT_HOURS_OF_OPERATION_NAME`]) those queues are attached to.
///
/// Every default resource is minted and stored exactly like a client-created one — a
/// fresh id, the instance-scoped ARN built from it, `now` as its modification instant —
/// so `DescribeQueue`, `ListQueues`, `DescribeContactFlow` and `ListContactFlows` all
/// answer for them from storage, and deleting any of them (or the instance) drops them.
/// Called only by `CreateInstance`: a read never provisions anything.
pub fn seed_instance_defaults(
    store: &mut ComAmazonawsConnectAmazonconnectserviceState,
    instance_id: &str,
    now: SystemTime,
) {
    // Every identifier is minted before the instance is borrowed, so the borrow below
    // is the only one.
    let hours_id = store.mint_resource_id();
    let queue_ids: Vec<String> = DEFAULT_QUEUE_NAMES
        .iter()
        .map(|_| store.mint_resource_id())
        .collect();
    let flow_ids: Vec<String> = DEFAULT_CONTACT_FLOWS
        .iter()
        .map(|_| store.mint_resource_id())
        .collect();

    let Some(instance) = store.instances.get_mut(instance_id) else {
        return;
    };
    let instance_arn = instance.arn.clone();
    let region = instance.region.clone();

    instance.hours_of_operations.insert(
        hours_id.clone(),
        HoursOfOperationRecord {
            id: hours_id.clone(),
            arn: hours_of_operation_arn(&instance_arn, &hours_id),
            name: DEFAULT_HOURS_OF_OPERATION_NAME.to_string(),
            description: Some(DEFAULT_HOURS_OF_OPERATION_DESCRIPTION.to_string()),
            time_zone: DEFAULT_HOURS_OF_OPERATION_TIME_ZONE.to_string(),
            config: default_hours_of_operation_config(),
            // The default hours of operation stands alone: no parent is provisioned
            // with it, so a read reports no `ParentHoursOfOperations`.
            parent_ids: Vec::new(),
            tags: BTreeMap::new(),
            last_modified_time: now,
            last_modified_region: region.clone(),
        },
    );

    for (name, id) in DEFAULT_QUEUE_NAMES.iter().zip(&queue_ids) {
        instance.queues.insert(
            id.clone(),
            QueueRecord {
                id: id.clone(),
                arn: format!("{instance_arn}/queue/{id}"),
                name: (*name).to_string(),
                description: Some(DEFAULT_QUEUE_DESCRIPTION.to_string()),
                hours_of_operation_id: hours_id.clone(),
                max_contacts: None,
                outbound_caller_config: None,
                outbound_email_config: None,
                quick_connect_ids: Vec::new(),
                queue_type: QUEUE_TYPE_STANDARD.to_string(),
                status: QUEUE_STATUS_ENABLED.to_string(),
                tags: BTreeMap::new(),
                last_modified_time: now,
                last_modified_region: region.clone(),
            },
        );
    }

    for (default, id) in DEFAULT_CONTACT_FLOWS.iter().zip(&flow_ids) {
        instance.contact_flows.insert(
            id.clone(),
            ContactFlowRecord {
                id: id.clone(),
                arn: format!("{instance_arn}/contact-flow/{id}"),
                name: default.name.to_string(),
                description: None,
                flow_type: default.flow_type.to_string(),
                // A default flow is one a contact can already be routed to, which is
                // what ACTIVE/PUBLISHED mean.
                state: CONTACT_FLOW_STATE_ACTIVE.to_string(),
                status: CONTACT_FLOW_STATUS_PUBLISHED.to_string(),
                content: DEFAULT_CONTACT_FLOW_CONTENT.to_string(),
                tags: BTreeMap::new(),
                last_modified_time: now,
                last_modified_region: region.clone(),
            },
        );
    }
}

/// The modelled `HoursOfOperationConfigList` the default hours of operation carries:
/// every day of the week, around the clock. The published default's own schedule is not
/// in the model, so the coverage is the emulator's choice; what matters is that the
/// default queue names hours of operation that exist.
fn default_hours_of_operation_config() -> Vec<HoursOfOperationConfig> {
    DEFAULT_HOURS_OF_OPERATION_DAYS
        .iter()
        .map(|day| HoursOfOperationConfig {
            day: (*day).to_string(),
            // Hours24Format range 0..23, MinutesLimit60 range 0..59.
            start_time: HoursOfOperationTimeSlice {
                hours: 0,
                minutes: 0,
            },
            end_time: HoursOfOperationTimeSlice {
                hours: 23,
                minutes: 59,
            },
        })
        .collect()
}

/// The checksum Connect reports for a flow's content: the SHA-256 of the content
/// bytes, lowercase hex — 64 characters, which is the only form the modelled
/// `FlowContentSha256` (`^[a-zA-Z0-9]{64}$`) can take. Derived from the stored
/// content, so the same content always checksums the same and a changed content
/// always changes the checksum.
pub fn flow_content_sha256(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope (an unsigned call, or one the emulator serves with
/// authentication off).
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:connect:<region>:<account>:instance/<id>`.
pub fn instance_arn(region: &str, instance_id: &str) -> String {
    ::engine::emulator_arn("connect", region, &format!("instance/{instance_id}"))
}

/// The service-linked role Connect reports for an instance, as an IAM ARN (IAM is
/// global, so the region segment is empty).
pub fn instance_service_role(instance_id: &str) -> String {
    ::engine::emulator_arn(
        "iam",
        "",
        &format!(
            "role/aws-service-role/connect.amazonaws.com/AWSServiceRoleForAmazonConnect_{instance_id}"
        ),
    )
}

/// The agent/admin console URL Connect reports for an instance. It is keyed on the
/// instance alias where there is one (`https://<alias>.my.connect.aws`), and on the
/// instance id otherwise, so the URL is always derived from stored state.
pub fn instance_access_url(alias: Option<&str>, instance_id: &str) -> String {
    let label = alias.unwrap_or(instance_id);
    format!("https://{label}.my.connect.aws")
}

/// Whether `value` has the identifier form
/// [`ComAmazonawsConnectAmazonconnectserviceState::mint_resource_id`] hands out: five
/// `-`-separated groups of 8, 4, 4, 4 and 12 hex digits.
///
/// The companion of the minter, and the reason a label the service could never have
/// minted is refused as a bad parameter rather than looked up: no stored resource can
/// carry an identifier of any other form, so such a label names no resource that could
/// exist, whatever the store holds. A handler asks this BEFORE it resolves anything,
/// which is what makes the refusal the bad parameter it is rather than a resource that
/// happens to be missing. The form is narrower than the `@length` bound the model puts
/// on `InstanceId` (1..100 characters,
/// `smithy_query(query="shape",shape="InstanceId")`), so a label outside that bound is
/// refused here too, and `QueueId`, which the model leaves unconstrained, is checked by
/// the same form.
pub fn is_resource_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(length, group)| {
                group.len() == *length && group.chars().all(|c| c.is_ascii_hexdigit())
            })
}

/// Convert a stored instant into the modelled timestamp type.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// Store a request's tags in deterministic order.
pub fn tags_in(tags: Option<HashMap<String, String>>) -> BTreeMap<String, String> {
    tags.unwrap_or_default().into_iter().collect()
}

/// Project stored tags back onto the wire. An untagged resource reports no `Tags`
/// member at all rather than an empty map, which is also the only form the modelled
/// `TagMap` (length 1..50) can take.
pub fn tags_out(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    if tags.is_empty() {
        None
    } else {
        Some(
            tags.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }
}

/// Encode a pagination cursor: the key of the last item on the page, in a form a
/// caller cannot read or construct meaning from.
pub fn encode_page_token(key: &str) -> String {
    hex::encode(key.as_bytes())
}

/// Decode a cursor produced by [`encode_page_token`]. `None` for anything else, which
/// the caller refuses as an invalid request rather than silently restarting the scan.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// Cut one page out of a key-ordered list.
///
/// `entries` is every matching item paired with its sort key, in ascending key order;
/// `after` is the decoded cursor (exclusive). The returned token is present only when
/// items remain beyond this page, so the last page carries none.
pub fn paginate<T>(
    entries: Vec<(String, T)>,
    after: Option<&str>,
    max_results: Option<i32>,
    default_page_size: usize,
) -> (Vec<T>, Option<String>) {
    let page_size = max_results
        .filter(|requested| *requested > 0)
        .map_or(default_page_size, |requested| requested as usize);

    let mut items = Vec::new();
    let mut last_key: Option<String> = None;
    let mut truncated = false;

    for (key, item) in entries {
        if after.is_some_and(|cursor| key.as_str() <= cursor) {
            continue;
        }
        if items.len() >= page_size {
            truncated = true;
            break;
        }
        last_key = Some(key);
        items.push(item);
    }

    let next_token = if truncated {
        last_key.as_deref().map(encode_page_token)
    } else {
        None
    };
    (items, next_token)
}

/// Whether the account's instance registry holds an instance under
/// `connect_instance_id`: the ONE read of this registry by a crate other than this one.
///
/// Amazon Connect owns the instance registry — `CreateInstance` is the only call that
/// mints an instance id and `DeleteInstance` the only one that drops it — so a service
/// whose operations require a usable instance resolves the id HERE rather than keeping a
/// registry of its own. It resolves by instance id alone, which is how every operation
/// that takes an `InstanceId` resolves one, and reading it changes nothing a later caller
/// sees.
///
/// `None` when this store cannot be reached at all (the engine is not the shared in-memory
/// one), which a caller answers with a modelled server error exactly as it answers its own
/// store being unavailable — never by treating the instance as absent.
///
/// LOCK ORDER: this takes THIS store's lock, holds no other while it does, and hands back
/// an owned value rather than a guard — so a caller asks it with NO guard of its own store
/// held, which keeps the cross-service read one-directional and a lock cycle between the
/// two stores unreachable.
pub fn registry_holds_instance<Eng: Send + Sync + 'static>(
    engine: &Eng,
    connect_instance_id: &str,
) -> Option<bool> {
    let state = service_state(engine)?;
    let holds = state.lock().instances.contains_key(connect_instance_id);
    Some(holds)
}

/// The modelled `DirectoryType` an instance [`register_instance`] writes carries: the
/// value for an instance Amazon Connect manages itself, from the catalog
/// `crate::ops::types::DIRECTORY_TYPE_VALUES` declares.
pub const REGISTERED_INSTANCE_DIRECTORY_TYPE: &str = "CONNECT_MANAGED";

/// Give the account an instance with id `connect_instance_id` in `region`, reporting
/// whether the registry holds that instance once this returns.
///
/// The WRITE seam of [`registry_holds_instance`], and the companion of
/// [`InstanceRecord::new`]: a crate other than this one that needs the account to hold an
/// instance under an id IT chose — rather than the id `CreateInstance` mints — registers it
/// through this one function, so the record such a registration writes is stated once and
/// grows with the record itself. The record is the one `CreateInstance` writes: aliased by
/// its own id, with [`REGISTERED_INSTANCE_DIRECTORY_TYPE`], and already past the clock-owed
/// activation instant ([`INSTANCE_ACTIVATION_DELAY`]), so the instance is `ACTIVE`.
///
/// Idempotent: an id the registry already holds keeps the record it holds and this answers
/// `true`. It answers `false` only when this store cannot be reached, or when the registry
/// is already at [`INSTANCE_QUOTA`] — the bound every writer of this map keeps.
///
/// LOCK ORDER: as [`registry_holds_instance`].
pub fn register_instance<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    connect_instance_id: &str,
) -> bool {
    let Some(state) = service_state(engine) else {
        return false;
    };
    let mut store = state.lock();
    if store.instances.contains_key(connect_instance_id) {
        return true;
    }
    if store.instances.len() >= INSTANCE_QUOTA {
        return false;
    }
    let now = SystemTime::now();
    store.instances.insert(
        connect_instance_id.to_string(),
        InstanceRecord::new(
            region,
            connect_instance_id,
            REGISTERED_INSTANCE_DIRECTORY_TYPE,
            Some(connect_instance_id.to_string()),
            now,
            now,
        ),
    );
    true
}

/// Take the account's instance `connect_instance_id` back out of the registry, reporting
/// whether a record was removed — the effect `DeleteInstance` has on it, for a crate other
/// than this one that needs an id the account no longer holds.
///
/// LOCK ORDER: as [`registry_holds_instance`].
pub fn deregister_instance<Eng: Send + Sync + 'static>(
    engine: &Eng,
    connect_instance_id: &str,
) -> bool {
    let Some(state) = service_state(engine) else {
        return false;
    };
    let mut store = state.lock();
    let removed = store.instances.remove(connect_instance_id).is_some();
    if removed {
        // The same effect `DeleteInstance` has: the identifier named an instance of this
        // account, so it stays behind as a retired one and a later call carrying it reports
        // the instance that is gone rather than one this account never held.
        store.retire_instance(connect_instance_id);
    }
    removed
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsConnectAmazonconnectserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsConnectAmazonconnectserviceState>)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The retirement record remembers the identifiers a delete retired, newest kept: it
    /// never holds more than the bound, and once it is full the oldest identifier is the
    /// one that leaves.
    ///
    /// The rows walk the record's size either side of its bound, so a repeated
    /// create-and-delete cannot grow the store without end.
    #[test]
    fn the_retirement_record_holds_at_most_its_bound() {
        let identifier = |index: usize| format!("instance-{index}");
        for retired in [
            0,
            1,
            RETIRED_INSTANCE_RECORD - 1,
            RETIRED_INSTANCE_RECORD,
            RETIRED_INSTANCE_RECORD + 1,
            RETIRED_INSTANCE_RECORD * 2,
        ] {
            let mut store = ComAmazonawsConnectAmazonconnectserviceState::default();
            for index in 0..retired {
                store.retire_instance(&identifier(index));
                // Retiring the same identifier again is not a second entry.
                store.retire_instance(&identifier(index));
            }
            let held = store.retired_instances.len();
            assert_eq!(
                held,
                retired.min(RETIRED_INSTANCE_RECORD),
                "case {retired} retired: the record holds at most {RETIRED_INSTANCE_RECORD}, got {held}"
            );
            // The identifiers still recorded are the newest ones; everything older has left.
            for index in 0..retired {
                let expected = index + RETIRED_INSTANCE_RECORD >= retired;
                let actual = store.is_retired_instance(&identifier(index));
                assert_eq!(
                    actual, expected,
                    "case {retired} retired: identifier {index} recorded={actual}, expected \
                     {expected}"
                );
            }
            assert!(
                !store.is_retired_instance(&identifier(retired)),
                "case {retired} retired: an identifier no delete retired is not in the record"
            );
        }
    }
}
