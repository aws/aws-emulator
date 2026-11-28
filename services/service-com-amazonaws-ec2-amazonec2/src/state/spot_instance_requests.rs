// SPDX-License-Identifier: CC0-1.0

//! The Spot instance request store: the request record, the states the clock owes it and the
//! bytes it retains.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

// The modelled shape an IAM instance profile reference is reported as
// (`IamInstanceProfileAssociation$IamInstanceProfile`), which a request stores the resolved
// profile of its launch specification as.
use crate::ops::types::IamInstanceProfile;

/// "`open` - The request is waiting to be fulfilled" — the `SpotInstanceState` enum's
/// `open` member, which is where every request a `RequestSpotInstances` mints starts.
pub const SPOT_REQUEST_STATE_OPEN: &str = "open";

/// "`active` - The request is fulfilled and has an associated Spot Instance" — the
/// `SpotInstanceState` enum's `active` member.
pub const SPOT_REQUEST_STATE_ACTIVE: &str = "active";

/// "`closed` - The request is unfulfillable" — the `SpotInstanceState` enum's `closed`
/// member, which an expired request lands in.
pub const SPOT_REQUEST_STATE_CLOSED: &str = "closed";

/// "`cancelled` - The request was canceled" — the `SpotInstanceState` enum's `cancelled`
/// member.
pub const SPOT_REQUEST_STATE_CANCELLED: &str = "cancelled";

/// The status code of a request that has been submitted and not yet evaluated, the first
/// of the Spot request status codes `SpotInstanceStatus$Code` points at.
pub const SPOT_STATUS_PENDING_EVALUATION: &str = "pending-evaluation";

/// The status code of a request whose instance has been launched.
pub const SPOT_STATUS_FULFILLED: &str = "fulfilled";

/// The status code of a request whose own validity window ended before it was fulfilled.
pub const SPOT_STATUS_SCHEDULE_EXPIRED: &str = "schedule-expired";

/// The status code of a request that could not be fulfilled for want of capacity.
pub const SPOT_STATUS_CAPACITY_NOT_AVAILABLE: &str = "capacity-not-available";

/// The status code of a request whose maximum price is below the Spot price in force: "if you
/// specify a maximum price, your instances will be interrupted more frequently"
/// (`RequestSpotInstancesRequest$SpotPrice`), and a maximum below the market price buys no
/// capacity at all, so the request stays `open` carrying this code and launches nothing.
pub const SPOT_STATUS_PRICE_TOO_LOW: &str = "price-too-low";

/// The description [`SPOT_STATUS_PENDING_EVALUATION`] carries: "The description for the
/// status code" (`SpotInstanceStatus$Message`). An EXAMPLE wording of the code, which is
/// what clients branch on.
pub const SPOT_STATUS_MESSAGE_PENDING_EVALUATION: &str =
    "Your Spot request has been submitted for review, and is pending evaluation.";

/// The description [`SPOT_STATUS_FULFILLED`] carries.
pub const SPOT_STATUS_MESSAGE_FULFILLED: &str = "Your Spot request is fulfilled.";

/// The description [`SPOT_STATUS_SCHEDULE_EXPIRED`] carries.
pub const SPOT_STATUS_MESSAGE_SCHEDULE_EXPIRED: &str =
    "Your Spot request has expired because the end of its validity period was reached.";

/// The description [`SPOT_STATUS_CAPACITY_NOT_AVAILABLE`] carries.
pub const SPOT_STATUS_MESSAGE_CAPACITY_NOT_AVAILABLE: &str =
    "There is no Spot capacity available that matches your request.";

/// The description [`SPOT_STATUS_PRICE_TOO_LOW`] carries, as `(bid, market)` fills it: the
/// maximum price the request named and the price fulfilling it would take, both read off the
/// request and this crate's one Spot price ([`spot_market_price_micros`]) rather than written
/// down here.
#[must_use]
pub fn spot_status_message_price_too_low(bid: &str, market: &str) -> String {
    format!(
        "Your Spot request price of {bid} is lower than the minimum required Spot request \
         fulfillment price of {market}."
    )
}

/// The `canceled-before-fulfillment` Spot request status code, one of the Spot request status
/// codes `SpotInstanceStatus$Code` points at: a request cancelled while it was still waiting
/// to be fulfilled.
pub const SPOT_STATUS_CANCELED_BEFORE_FULFILLMENT: &str = "canceled-before-fulfillment";

/// The description [`SPOT_STATUS_CANCELED_BEFORE_FULFILLMENT`] carries.
pub const SPOT_STATUS_MESSAGE_CANCELED_BEFORE_FULFILLMENT: &str =
    "Your Spot request was canceled before it was fulfilled.";

/// The `request-canceled-and-instance-running` Spot request status code: a fulfilled request
/// that was cancelled while the instance it launched was left running.
pub const SPOT_STATUS_REQUEST_CANCELED_AND_INSTANCE_RUNNING: &str =
    "request-canceled-and-instance-running";

/// The description [`SPOT_STATUS_REQUEST_CANCELED_AND_INSTANCE_RUNNING`] carries.
pub const SPOT_STATUS_MESSAGE_REQUEST_CANCELED_AND_INSTANCE_RUNNING: &str =
    "Your Spot request is canceled, but your instance is still running.";

/// The `instance-terminated-by-user` Spot request status code: the instance the request
/// launched was terminated on a caller's own instruction.
pub const SPOT_STATUS_INSTANCE_TERMINATED_BY_USER: &str = "instance-terminated-by-user";

/// The description [`SPOT_STATUS_INSTANCE_TERMINATED_BY_USER`] carries.
pub const SPOT_STATUS_MESSAGE_INSTANCE_TERMINATED_BY_USER: &str =
    "Your Spot Instance was terminated at your request.";

/// The id prefix of a Spot Instance request, the form
/// `SpotInstanceRequest$SpotInstanceRequestId` carries.
pub const SPOT_INSTANCE_REQUEST_ID_PREFIX: &str = "sir";

/// [`SPOT_INSTANCE_REQUEST_ID_PREFIX`] and the hyphen that separates it from the id's hex
/// suffix, as one static, so recognising an id allocates nothing on the request path.
pub const SPOT_INSTANCE_REQUEST_ID_PREFIX_WITH_HYPHEN: &str = "sir-";

/// "`one-time`" — the `SpotInstanceType` enum's `one-time` member, and the type
/// `RequestSpotInstancesRequest$Type` documents as its default ("Default: one-time").
pub const SPOT_INSTANCE_TYPE_ONE_TIME: &str = "one-time";

/// "`persistent`" — the `SpotInstanceType` enum's other member.
pub const SPOT_INSTANCE_TYPE_PERSISTENT: &str = "persistent";

/// The whole `SpotInstanceType` catalog, which the `type` filter selects over.
pub const SPOT_INSTANCE_TYPES: [&str; 2] = [
    SPOT_INSTANCE_TYPE_ONE_TIME,
    SPOT_INSTANCE_TYPE_PERSISTENT,
];

/// "The behavior when a Spot Instance is interrupted. The default is terminate"
/// (`RequestSpotInstancesRequest$InstanceInterruptionBehavior`): the
/// `InstanceInterruptionBehavior` enum's `terminate` member.
pub const INSTANCE_INTERRUPTION_BEHAVIOR_TERMINATE: &str = "terminate";

/// The resource type a `RequestSpotInstances` `TagSpecification` may name: "The value for
/// ResourceType must be spot-instances-request, otherwise the Spot Instance request fails"
/// (`RequestSpotInstancesRequest$TagSpecifications`).
pub const SPOT_REQUEST_TAGGABLE_RESOURCE: &str = "spot-instances-request";

/// How long a Spot Instance request stays valid when the request names no `ValidUntil`.
///
/// DOCUMENTED, not chosen: "By default, the request is valid for 7 days from the date the
/// request was created" (`RequestSpotInstancesRequest$ValidUntil`).
pub const DEFAULT_SPOT_REQUEST_VALIDITY: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How many Spot Instances one `RequestSpotInstances` call may ask for.
///
/// CHOSEN for the same reason as [`MAX_INSTANCES_PER_RUN`], and equal to it: "the maximum
/// number of Spot Instances to launch" (`RequestSpotInstancesRequest$InstanceCount`)
/// arrives as a plain integer with no modelled `@range`, and one request mints a stored
/// record — and later an instance — per unit of it.
pub const MAX_SPOT_INSTANCES_PER_REQUEST: i32 = MAX_INSTANCES_PER_RUN;

/// How many Spot Instance requests this emulator stores before `RequestSpotInstances` is
/// refused.
///
/// CHOSEN, like [`MAX_STORED_IMPORT_IMAGE_TASKS`] and for the same reason: nothing here
/// reclaims a closed request, so the store needs a ceiling that is not the process's
/// memory. The caller-chosen BYTES of each request are charged against
/// [`MAX_STORED_BYTES`] separately ([`spot_instance_request_payload_bytes`]).
pub const MAX_STORED_SPOT_INSTANCE_REQUESTS: usize = 4_096;

/// "`spot` - the instance is a Spot Instance" — the `InstanceLifecycleType` enum's `spot`
/// member, which is what an instance launched to fulfill a Spot Instance request reports
/// as its `InstanceLifecycle`.
pub const INSTANCE_LIFECYCLE_SPOT: &str = "spot";

/// The `MonitoringState` enum's `enabled` member, which an instance launched with detailed
/// monitoring reports.
pub const MONITORING_STATE_ENABLED: &str = "enabled";

/// The `MonitoringState` enum's `disabled` member, which an instance launched without
/// detailed monitoring reports ("Default: Disabled",
/// `RequestSpotLaunchSpecification$Monitoring`).
pub const MONITORING_STATE_DISABLED: &str = "disabled";

/// The `MonitoringState` enum's `pending` member: the transient state detailed monitoring
/// is in between `MonitorInstances` and the `enabled` the clock then owes it
/// ([`ComAmazonawsEc2Amazonec2State::set_instance_monitoring`]).
pub const MONITORING_STATE_PENDING: &str = "pending";

/// The `MonitoringState` enum's `disabling` member: the transient state on the way down,
/// between `UnmonitorInstances` and the `disabled` the clock then owes it.
pub const MONITORING_STATE_DISABLING: &str = "disabling";

/// The product description of a Spot request for a Linux AMI: "`product-description` - The
/// product description associated with the instance (`Linux/UNIX` | `Windows`)"
/// (`DescribeSpotInstanceRequestsRequest$Filters`), and a member of the modelled
/// `RIProductDescription` catalog.
pub const PRODUCT_DESCRIPTION_LINUX_UNIX: &str = "Linux/UNIX";

/// The product description of a Spot request for a Windows AMI, from the same sentence.
pub const PRODUCT_DESCRIPTION_WINDOWS: &str = IMAGE_PLATFORM_WINDOWS;

/// EC2's code for a Spot Instance request id no request of the account has.
pub const INVALID_SPOT_INSTANCE_REQUEST_ID_NOT_FOUND: &str =
    "InvalidSpotInstanceRequestID.NotFound";

/// EC2's code for something that is not a Spot Instance request id at all.
pub const INVALID_SPOT_INSTANCE_REQUEST_ID_MALFORMED: &str =
    "InvalidSpotInstanceRequestID.Malformed";

/// The furthest ahead this emulator carries an instant computed from a caller-supplied
/// date: a century.
///
/// `Instant + Duration` and `SystemTime + Duration` PANIC when the sum is not
/// representable, and a `ValidFrom` / `ValidUntil` is an unbounded timestamp off a single
/// request, so every due instant computed from one is capped here and added with CHECKED
/// arithmetic ([`due_instant`], [`due_system_time`]). A request valid for longer than this
/// process will ever run is indistinguishable from one valid for a century, so the cap
/// changes nothing a caller can observe — and no request can reach the overflow.
pub const MAX_CLOCK_HORIZON: Duration = Duration::from_secs(100 * 365 * 24 * 60 * 60);

/// `Instant::now()` plus `delay`, capped at [`MAX_CLOCK_HORIZON`] and added with checked
/// arithmetic: a delay this platform's monotonic clock cannot represent is halved until it
/// can, so a caller-supplied date can never panic a handler.
pub fn due_instant(delay: Duration) -> Instant {
    let now = Instant::now();
    let mut wanted = delay.min(MAX_CLOCK_HORIZON);
    loop {
        match now.checked_add(wanted) {
            Some(due_at) => return due_at,
            None if wanted.is_zero() => return now,
            None => wanted /= 2,
        }
    }
}

/// The wall-clock counterpart of [`due_instant`]: `base` plus `delay`, capped and checked
/// the same way, for the instant a move is reported to have fallen due.
pub fn due_system_time(base: SystemTime, delay: Duration) -> SystemTime {
    let mut wanted = delay.min(MAX_CLOCK_HORIZON);
    loop {
        match base.checked_add(wanted) {
            Some(at) => return at,
            None if wanted.is_zero() => return base,
            None => wanted /= 2,
        }
    }
}

/// A move the clock owes one Spot Instance request, with the instants it falls due on.
///
/// The same shape, and the same reasons for both fields, as [`FastLaunchTransition`]:
/// `due_at` is monotonic because it is only ever compared against the clock, and `at` is
/// the wall-clock instant the move fell due, which the request reports as its status
/// `UpdateTime` once the move has been applied.
#[derive(Debug, Clone)]
pub struct SpotTransition {
    /// The instant from which the move is due.
    pub due_at: Instant,
    /// The wall-clock instant the move falls due.
    pub at: SystemTime,
}

impl SpotTransition {
    /// A move due `delay` from now, on both clocks.
    ///
    /// `delay` can come from a caller's `ValidFrom`, so both instants are computed by the
    /// capped, checked helpers rather than by the panicking `+`.
    pub fn due_in(delay: Duration) -> Self {
        Self {
            due_at: due_instant(delay),
            at: due_system_time(SystemTime::now(), delay),
        }
    }
}

/// The bytes one stored task holds: its fixed overhead
/// ([`STORE_IMAGE_TASK_RECORD_BYTES`]) plus the one caller-chosen string it keeps, the
/// bucket name. The AMI id and the object key are derived from it, so they are a fixed
/// size.
///
/// The ONE definition of that charge, the way [`address_payload_bytes`] is for an address:
/// a writer charges exactly it, and a removal path gives back the same number.
pub fn store_image_task_payload_bytes(record: &StoreImageTaskRecord) -> usize {
    store_image_task_payload_bytes_for(&record.bucket)
}

/// What [`store_image_task_payload_bytes`] will charge a task targeting `bucket`, BEFORE
/// one is minted — so a create can ask the byte budget without building a record only to
/// measure it.
pub fn store_image_task_payload_bytes_for(bucket: &str) -> usize {
    STORE_IMAGE_TASK_RECORD_BYTES.saturating_add(bucket.len())
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another store task fits inside the store's ceiling
    /// ([`MAX_STORE_IMAGE_TASKS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// with [`store_image_task_limit_exceeded`] when it answers `false`.
    pub fn has_store_image_task_capacity(&self) -> bool {
        self.store_image_tasks.len() < MAX_STORE_IMAGE_TASKS
    }

    /// Start a task storing `ami_id` into `bucket`, and answer the internal key it was
    /// stored under.
    ///
    /// Everything the task reports beyond the bucket is derived here rather than taken from
    /// the caller — the object key from the AMI's id, the start time from the clock, and the
    /// state and progress from the task's age — so two readers cannot report one task
    /// differently. The task starts out on its way to `Completed`: the upload the create then
    /// makes is what can leave it `Failed` instead
    /// ([`Self::fail_store_image_task`]).
    ///
    /// EVERY call records a task: "you get a paginated list of store tasks from the last 31
    /// days" (`DescribeStoreImageTasks`) is every task started, and AWS publishes no
    /// de-duplication, so storing one AMI into one bucket again is a SECOND task — minted a
    /// later key, which is what puts it first in the reverse-chronological report — rather
    /// than something that drops a task a caller could still describe.
    pub fn insert_store_image_task(&mut self, ami_id: &str, bucket: &str) -> String {
        let key = self.next_id(STORE_IMAGE_TASK_KEY_PREFIX);
        let record = StoreImageTaskRecord::starting_now(ami_id, bucket);
        self.add_stored_bytes(store_image_task_payload_bytes(&record));
        self.store_image_tasks.insert(key.clone(), record);
        key
    }

    /// Record that the task held under `key` did not store its AMI object, for the `reason`
    /// the object store answered the upload with ([`write_stored_ami_object`]).
    ///
    /// The upload is made with no guard of this store held, so its outcome is written back
    /// here — the one writer of `StoreImageTaskResult$StoreTaskFailureReason`. A key no task
    /// is held under is one a later caller cannot see either, so there is nothing to record.
    pub fn fail_store_image_task(&mut self, key: &str, reason: String) {
        if let Some(record) = self.store_image_tasks.get_mut(key) {
            record.failure_reason = Some(reason);
        }
    }

    /// Drop every store task whose window has run out, as of `now`.
    ///
    /// "Currently, only tasks from the past 31 days can be viewed"
    /// (`DescribeStoreImageTasks`): a task leaves the account's view
    /// [`STORE_IMAGE_TASK_VISIBILITY`] after it started, which the clock owes it whether or
    /// not anybody ever calls again — so
    /// [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] calls this, and the
    /// caller-chosen bytes the task held are given back.
    pub fn expire_due_store_image_tasks(&mut self, now: Instant) {
        let expired: Vec<String> = self
            .store_image_tasks
            .iter()
            .filter(|(_, record)| now >= record.leaves_view_at)
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            if let Some(record) = self.store_image_tasks.remove(&key) {
                self.release_stored_bytes(store_image_task_payload_bytes(&record));
            }
        }
    }
}

/// One stored Spot Instance request: the modelled [`SpotInstanceRequest`] every operation
/// that reports one answers with, plus the facts that shape carries no member for.
///
/// The stored view is the truth about the request — `RequestSpotInstances` answers it and
/// `DescribeSpotInstanceRequests` reports it, so the two cannot describe one request
/// differently — and every clock-owed move is written onto it ONCE
/// ([`ComAmazonawsEc2Amazonec2State::materialize_due_spot_instance_requests`]), because a
/// fulfillment MINTS an instance that has to be the same in every later read.
#[derive(Debug, Clone)]
pub struct SpotInstanceRequestRecord {
    /// The request as the model shapes it, carrying the state reached so far.
    pub view: SpotInstanceRequest,
    /// The fulfillment the clock owes this request while it is `open`, or `None` once it
    /// has been fulfilled, has expired or could not be fulfilled at all.
    pub fulfillment: Option<SpotTransition>,
    /// The instant this request stops being valid, on the monotonic clock: the
    /// `ValidUntil` the view reports, which is compared against the clock here and never
    /// reported from here, so a wall-clock adjustment cannot un-expire a request.
    pub valid_until_at: Instant,
    /// The Region the request was made in, which the instance a fulfillment launches
    /// needs for its private DNS name. Not a member of any modelled Spot shape.
    pub region: String,
    /// The IAM instance profile the instance a fulfillment launches RUNS AS, as the request's
    /// launch specification named it and the account's IAM records resolved it
    /// ([`resolve_iam_instance_profile`]) — or `None` for a specification that named none.
    ///
    /// Resolved by the CALL rather than by the fulfillment, because resolving it is an ask of
    /// the service that owns the profile; what the fulfillment then does with it is record the
    /// association over the instance it launches, through the one writer of one
    /// ([`ComAmazonawsEc2Amazonec2State::insert_iam_instance_profile_association`]).
    pub iam_instance_profile: Option<IamInstanceProfile>,
}

/// The caller-chosen bytes one stored Spot Instance request holds: the launch
/// specification's user data, the ids and names the launch named, the zone it placed the
/// instance in, its tags, the launch and Availability Zone groups, the maximum price and
/// the `ClientToken` the token map keeps for it. Everything else on the record — the id,
/// the state, the status code, the type and the interruption behaviour — is derived here or
/// drawn from a closed catalog, so it is fixed-size.
///
/// The ONE definition of that charge: `RequestSpotInstances` asks it before the write and
/// charges exactly it, per request it mints. It also COVERS the caller-chosen bytes of the
/// instance a fulfillment later launches ([`instance_payload_bytes`]), which is what bounds
/// that launch's own charge: the instance carries no caller-chosen member its request does
/// not — and the one member it can hold that the request did not name, the zone the service
/// picks when the request named none, is a name out of the Region's own zone set rather
/// than anything a caller sized. The block device mappings the specification carries are
/// charged here too, every caller-chosen string on each of them
/// ([`super::block_device_mapping_bytes`]): the record RETAINS the mappings, and the instance a
/// fulfillment launches retains the root block device they resolve to
/// ([`ComAmazonawsEc2Amazonec2State::launch_root_block_device`]).
pub fn spot_instance_request_payload_bytes(
    view: &SpotInstanceRequest,
    client_token: Option<&str>,
) -> usize {
    let launch = view.launch_specification.as_ref();
    // The zone the instance is placed in: named by the request when it names one, and
    // retained on the instance a fulfillment launches, so it is charged here too.
    let placement_bytes = launch
        .and_then(|launch| launch.placement.as_ref())
        .and_then(|placement| placement.availability_zone.as_ref())
        .map_or(0, String::len);
    let group_bytes: usize = launch.map_or(0, |launch| {
        launch
            .security_groups
            .iter()
            .flatten()
            .map(|group| {
                group.group_id.as_ref().map_or(0, String::len)
                    + group.group_name.as_ref().map_or(0, String::len)
            })
            .sum()
    });
    let mapping_bytes: usize = launch.map_or(0, |launch| {
        launch
            .block_device_mappings
            .iter()
            .flatten()
            .map(super::block_device_mapping_bytes)
            .fold(0usize, usize::saturating_add)
    });
    // The IAM instance profile the specification named, which the record retains as given and
    // the instance a fulfillment launches retains resolved.
    let profile_bytes: usize = launch.map_or(0, |launch| {
        launch
            .iam_instance_profile
            .as_ref()
            .map_or(0, |profile| {
                profile.arn.as_ref().map_or(0, String::len)
                    + profile.name.as_ref().map_or(0, String::len)
            })
            .saturating_mul(2)
    });
    launch
        .and_then(|launch| launch.user_data.as_ref())
        .map_or(0, String::len)
        .saturating_add(
            launch
                .and_then(|launch| launch.image_id.as_ref())
                .map_or(0, String::len),
        )
        .saturating_add(
            launch
                .and_then(|launch| launch.key_name.as_ref())
                .map_or(0, String::len),
        )
        .saturating_add(
            launch
                .and_then(|launch| launch.subnet_id.as_ref())
                .map_or(0, String::len),
        )
        .saturating_add(
            launch
                .and_then(|launch| launch.instance_type.as_ref())
                .map_or(0, String::len),
        )
        .saturating_add(group_bytes)
        .saturating_add(mapping_bytes)
        .saturating_add(profile_bytes)
        .saturating_add(placement_bytes)
        .saturating_add(tag_list_bytes(view.tags.as_deref().unwrap_or_default()))
        .saturating_add(view.launch_group.as_ref().map_or(0, String::len))
        .saturating_add(view.availability_zone_group.as_ref().map_or(0, String::len))
        .saturating_add(view.spot_price.as_ref().map_or(0, String::len))
        .saturating_add(client_token.map_or(0, str::len))
}

/// Whether `id` is a well-formed Spot Instance request id.
///
/// `sir-` followed by 8 or 17 lowercase hexadecimal digits — the short and the long id
/// form every EC2 resource id takes, read the same way
/// [`import_image_task_id_is_well_formed`] reads an import task's.
pub fn spot_instance_request_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix(SPOT_INSTANCE_REQUEST_ID_PREFIX_WITH_HYPHEN) else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Refuse something that is not a Spot Instance request id at all: HTTP 400
/// [`INVALID_SPOT_INSTANCE_REQUEST_ID_MALFORMED`]. There is nothing to look up for it, so
/// it is refused before any lookup, the way this service answers every malformed resource
/// id.
pub fn require_well_formed_spot_instance_request_id<E>(
    request_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if spot_instance_request_id_is_well_formed(request_id) {
        return Ok(());
    }
    Err(crate::scaffolded::Scaffolded::service_error(
        INVALID_SPOT_INSTANCE_REQUEST_ID_MALFORMED,
        400,
        invalid_id_message(request_id),
    ))
}

/// Refuse a request that named a Spot Instance request id no request of the account has:
/// HTTP 400 [`INVALID_SPOT_INSTANCE_REQUEST_ID_NOT_FOUND`].
pub fn invalid_spot_instance_request_id_not_found<E>(
    request_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_SPOT_INSTANCE_REQUEST_ID_NOT_FOUND,
        400,
        format!("The spot instance request ID '{request_id}' does not exist"),
    )
}

/// A modelled timestamp as a wall-clock instant: the inverse of
/// [`system_time_to_date_time`], for a `ValidFrom` / `ValidUntil` the caller named and the
/// service has to compare against the clock.
pub fn date_time_to_system_time(at: ::aws_smithy_types::DateTime) -> SystemTime {
    let offset = Duration::new(at.secs().unsigned_abs(), at.subsec_nanos());
    if at.secs() >= 0 {
        UNIX_EPOCH.checked_add(offset).unwrap_or(UNIX_EPOCH)
    } else {
        UNIX_EPOCH.checked_sub(offset).unwrap_or(UNIX_EPOCH)
    }
}

/// How far `at` is in the future, or [`Duration::ZERO`] when it is not in the future.
pub fn duration_until(at: SystemTime) -> Duration {
    at.duration_since(SystemTime::now()).unwrap_or_default()
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another Spot Instance request fits inside the store's ceiling
    /// ([`MAX_STORED_SPOT_INSTANCE_REQUESTS`]).
    ///
    /// The same contract as [`Self::has_import_image_task_capacity`]: a create asks first,
    /// for every request the call would mint, and refuses instead of growing the store
    /// past its ceiling.
    pub fn has_spot_instance_request_capacity(&self, wanted: usize) -> bool {
        self.spot_instance_requests
            .len()
            .saturating_add(wanted)
            <= MAX_STORED_SPOT_INSTANCE_REQUESTS
    }

    /// Store one Spot Instance request under a freshly minted id, and answer that id.
    ///
    /// The one writer of a Spot Instance request: the id comes from the store's own mint,
    /// the view's own `SpotInstanceRequestId` is set from it, and the record's bytes are
    /// charged against [`MAX_STORED_BYTES`] by the one definition of that charge, so no
    /// caller can store a request the byte budget has not been told about.
    ///
    /// `iam_instance_profile` is the profile the instance a fulfillment launches runs as,
    /// already resolved against the account's IAM records by the caller — `None` for a launch
    /// specification that named none, which is every launch a fleet places.
    pub fn insert_spot_instance_request(
        &mut self,
        mut view: SpotInstanceRequest,
        fulfillment: Option<SpotTransition>,
        valid_until_at: Instant,
        region: &str,
        client_token: Option<&str>,
        iam_instance_profile: Option<IamInstanceProfile>,
    ) -> String {
        let request_id = self.next_id(SPOT_INSTANCE_REQUEST_ID_PREFIX);
        view.spot_instance_request_id = Some(request_id.clone());
        self.add_stored_bytes(spot_instance_request_payload_bytes(&view, client_token));
        self.spot_instance_requests.insert(
            request_id.clone(),
            SpotInstanceRequestRecord {
                view,
                fulfillment,
                valid_until_at,
                region: region.to_string(),
                iam_instance_profile,
            },
        );
        request_id
    }

    /// Apply every move the clock owed a Spot Instance request by `now`.
    ///
    /// Reached only through [`Self::materialize_due_transitions`], so a reporting operation
    /// observes these moves without having caused them: each was owed at this instant
    /// whether or not anybody called. Two moves, in this order, because a request whose
    /// validity ended before its fulfillment fell due was never fulfillable:
    ///
    /// 1. EXPIRY — "for a one-time request, the request remains active until all instances
    ///    launch, the request is canceled, or the `validUntil` date and time is reached"
    ///    (`SpotInstanceRequest$ValidUntil`): an `open` request whose window has closed is
    ///    [`SPOT_REQUEST_STATE_CLOSED`] with the status code
    ///    [`SPOT_STATUS_SCHEDULE_EXPIRED`].
    /// 2. FULFILLMENT — "the instance ID, if an instance has been launched to fulfill the
    ///    Spot Instance request" (`SpotInstanceRequest$InstanceId`): the request becomes
    ///    [`SPOT_REQUEST_STATE_ACTIVE`] with the status code [`SPOT_STATUS_FULFILLED`] and
    ///    an instance of this account's own instance store. It is a WRITE rather than a
    ///    derive for the same reason an import's completion is: it mints an instance every
    ///    later read has to agree on.
    pub fn materialize_due_spot_instance_requests(&mut self, now: Instant) {
        let expired: Vec<String> = self
            .spot_instance_requests
            .iter()
            .filter(|(_, record)| {
                record.view.state.as_deref() == Some(SPOT_REQUEST_STATE_OPEN)
                    && now >= record.valid_until_at
            })
            .map(|(request_id, _)| request_id.clone())
            .collect();
        for request_id in expired {
            let Some(record) = self.spot_instance_requests.get_mut(&request_id) else {
                continue;
            };
            record.view.state = Some(SPOT_REQUEST_STATE_CLOSED.to_string());
            // The window closed when `ValidUntil` was reached, not when somebody next
            // asked, so that is the instant the status update reports.
            record.view.status = Some(SpotInstanceStatus {
                code: Some(SPOT_STATUS_SCHEDULE_EXPIRED.to_string()),
                message: Some(SPOT_STATUS_MESSAGE_SCHEDULE_EXPIRED.to_string()),
                update_time: record.view.valid_until,
            });
            // Nothing is owed on a request that can no longer be fulfilled.
            record.fulfillment = None;
        }

        let due: Vec<String> = self
            .spot_instance_requests
            .iter()
            .filter(|(_, record)| {
                record.view.state.as_deref() == Some(SPOT_REQUEST_STATE_OPEN)
                    && record
                        .fulfillment
                        .as_ref()
                        .is_some_and(|fulfillment| now >= fulfillment.due_at)
            })
            .map(|(request_id, _)| request_id.clone())
            .collect();
        for request_id in due {
            // A request a CALLER placed tags the request, not the instance it launches, so
            // the instance this fulfilment mints carries no tags of its own.
            self.fulfill_spot_instance_request(&request_id, BTreeMap::new());
        }
    }

    /// Launch the instance one due Spot Instance request asked for, and move the request to
    /// `active` / `fulfilled`.
    ///
    /// Every launch parameter comes off the request's own stored launch specification, so
    /// the instance is the one the request described, and the launch itself is the one every
    /// placement from a launch specification goes through
    /// ([`Self::launch_specified_instance`]) — so an instance a request launches and one an
    /// EC2 Fleet places are the same kind of instance. `tags` are the tags the instance
    /// itself carries: a request a CALLER placed tags the request ("the Spot Instance
    /// request on creation", `RequestSpotInstancesRequest$TagSpecifications`) and so gives
    /// the instance none, while a fleet's `instance` tag specifications tag "the instances
    /// at launch" (`CreateFleetRequest$TagSpecifications`).
    ///
    /// A request this store has no room to launch for — no instance slot, or no private
    /// address left — stays `open` carrying [`SPOT_STATUS_CAPACITY_NOT_AVAILABLE`], which
    /// is what the real service reports for a request it cannot place, and owes nothing
    /// further.
    ///
    /// The instance's own byte charge is made by [`Self::insert_instance`] and is NOT asked
    /// for again here, because `RequestSpotInstances` already asked for it: every
    /// caller-chosen string the instance holds is one
    /// [`spot_instance_request_payload_bytes`] charged for the request that launched it, so
    /// the store's byte ceiling was cleared before this move was ever owed.
    pub(super) fn fulfill_spot_instance_request(
        &mut self,
        request_id: &str,
        tags: BTreeMap<String, String>,
    ) {
        let Some(record) = self.spot_instance_requests.get(request_id) else {
            return;
        };
        let launch = record.view.launch_specification.clone().unwrap_or_default();
        let region = record.region.clone();
        let at = record
            .fulfillment
            .as_ref()
            .map_or_else(SystemTime::now, |fulfillment| fulfillment.at);

        // "The maximum price per unit hour that you are willing to pay for a Spot Instance"
        // (`RequestSpotInstancesRequest$SpotPrice`): a maximum BELOW the Spot price in force
        // buys no capacity, so the request is not fulfilled — it stays `open` carrying
        // [`SPOT_STATUS_PRICE_TOO_LOW`] and launches no instance. Asked at the instant the
        // fulfillment falls due, against the price this crate publishes for that zone, type
        // and product at that instant ([`spot_market_price_micros`], the same price
        // `DescribeSpotPriceHistory` reports), so a bid a client chose off the history is
        // judged against the price the history showed it.
        if let Some((bid, market)) = self.spot_bid_below_market(request_id, at) {
            self.leave_spot_instance_request_underpriced(request_id, at, &bid, &market);
            return;
        }

        let Some(launched) = self.launch_specified_instance(SpecifiedLaunch {
            launch: &launch,
            region: &region,
            // The instance was launched when the fulfillment fell due, not when somebody
            // next asked about it.
            at,
            spot_instance_request_id: Some(request_id),
            tags,
        }) else {
            self.leave_spot_instance_request_unplaced(request_id, at);
            return;
        };
        let LaunchedInstance {
            instance_id,
            availability_zone,
        } = launched;

        // "The IAM instance profile" the specification named
        // (`RequestSpotLaunchSpecification$IamInstanceProfile`): the instance this fulfillment
        // launched RUNS AS that profile, recorded over it as an association of its own through
        // the store's one writer of one — so the instance reports the profile off the same
        // record `DescribeIamInstanceProfileAssociations` reports, exactly as an on-demand
        // launch that named one does.
        if let Some(profile) = self
            .spot_instance_requests
            .get(request_id)
            .and_then(|record| record.iam_instance_profile.clone())
        {
            self.insert_iam_instance_profile_association(IamInstanceProfileAssociationSpec {
                instance_id: instance_id.clone(),
                iam_instance_profile: profile,
            });
        }

        if let Some(record) = self.spot_instance_requests.get_mut(request_id) {
            record.view.state = Some(SPOT_REQUEST_STATE_ACTIVE.to_string());
            record.view.status = Some(SpotInstanceStatus {
                code: Some(SPOT_STATUS_FULFILLED.to_string()),
                message: Some(SPOT_STATUS_MESSAGE_FULFILLED.to_string()),
                update_time: Some(system_time_to_date_time(at)),
            });
            record.view.instance_id = Some(instance_id.clone());
            // "The Availability Zone in which the request is launched" — known only now
            // that it has been launched. Its zone ID is deliberately NOT set beside it:
            // "either launchedAvailabilityZone or launchedAvailabilityZoneId can be
            // specified, but not both" (`SpotInstanceRequest$LaunchedAvailabilityZone`).
            record.view.launched_availability_zone = Some(availability_zone);
            record.fulfillment = None;
        }
        // "`launched` - a new instance was launched" is an event of the history of the Spot
        // Fleet request that placed this Spot Instance request, if a fleet placed it: the
        // launch happened when the fulfillment fell due, which is the instant it is recorded
        // at. A request a caller placed itself belongs to no fleet and records nothing.
        self.record_spot_fleet_instance_event(
            request_id,
            &instance_id,
            EVENT_SUB_TYPE_LAUNCHED,
            at,
        );
    }

    /// Leave one due Spot Instance request `open` carrying
    /// [`SPOT_STATUS_CAPACITY_NOT_AVAILABLE`]: there was no capacity in this store to
    /// place its instance in.
    pub(super) fn leave_spot_instance_request_unplaced(&mut self, request_id: &str, at: SystemTime) {
        if let Some(record) = self.spot_instance_requests.get_mut(request_id) {
            record.view.status = Some(SpotInstanceStatus {
                code: Some(SPOT_STATUS_CAPACITY_NOT_AVAILABLE.to_string()),
                message: Some(SPOT_STATUS_MESSAGE_CAPACITY_NOT_AVAILABLE.to_string()),
                update_time: Some(system_time_to_date_time(at)),
            });
            record.fulfillment = None;
        }
    }

    /// Leave one due Spot Instance request `open` carrying [`SPOT_STATUS_PRICE_TOO_LOW`]: the
    /// maximum price it named (`bid`) is below the Spot price fulfilling it would take
    /// (`market`), so no capacity was bought and no instance was launched.
    ///
    /// The request stays in [`SPOT_REQUEST_STATE_OPEN`] — "the request is waiting to be
    /// fulfilled" — and owes no further fulfillment: the price was evaluated at the instant
    /// the fulfillment fell due, and what ends such a request is its own validity window
    /// ([`Self::materialize_due_spot_instance_requests`]'s expiry, which still applies).
    pub(super) fn leave_spot_instance_request_underpriced(
        &mut self,
        request_id: &str,
        at: SystemTime,
        bid: &str,
        market: &str,
    ) {
        if let Some(record) = self.spot_instance_requests.get_mut(request_id) {
            record.view.status = Some(SpotInstanceStatus {
                code: Some(SPOT_STATUS_PRICE_TOO_LOW.to_string()),
                message: Some(spot_status_message_price_too_low(bid, market)),
                update_time: Some(system_time_to_date_time(at)),
            });
            record.fulfillment = None;
        }
    }

    /// The maximum price the Spot Instance request `request_id` named and the Spot price in
    /// force for it at `at`, as `(bid, market)` — or `None` when the bid is at or above that
    /// price, which is the request this store fulfills.
    ///
    /// `None` as well for a request that named NO maximum price: "if you do not specify this
    /// parameter, you will pay the current Spot price"
    /// (`RequestSpotInstancesRequest$SpotPrice`), so such a request is never priced out — and
    /// for one whose instance type this emulator publishes no facts for, which has no price to
    /// be judged against. The zone the price is read for is the one the launch would be placed
    /// in, read exactly as [`Self::launch_specified_instance`] reads it, so the request is
    /// judged against the price of the zone its instance would run in.
    fn spot_bid_below_market(
        &self,
        request_id: &str,
        at: SystemTime,
    ) -> Option<(String, String)> {
        let record = self.spot_instance_requests.get(request_id)?;
        let bid = record
            .view
            .spot_price
            .as_deref()
            .filter(|price| !price.is_empty())?;
        let bid_micros = hourly_price_micros(bid)?;
        let launch = record.view.launch_specification.as_ref()?;
        let instance_type = launch
            .instance_type
            .as_deref()
            .filter(|instance_type| !instance_type.is_empty())
            .unwrap_or(DEFAULT_INSTANCE_TYPE);
        let zone = launch
            .placement
            .as_ref()
            .and_then(|placement| placement.availability_zone.clone())
            .filter(|zone| !zone.is_empty())
            .or_else(|| {
                availability_zones_in_region(&record.region)
                    .first()
                    .map(|zone| zone.zone_name.clone())
            })
            .unwrap_or_else(|| record.region.clone());
        let product_description = record
            .view
            .product_description
            .as_deref()
            .unwrap_or(PRODUCT_DESCRIPTION_LINUX_UNIX);
        let market = spot_market_price_micros(
            &record.region,
            &zone,
            instance_type,
            product_description,
            at,
        )?;
        if bid_micros >= market {
            return None;
        }
        Some((bid.to_string(), format_price_micros(market)))
    }

    /// Cancel the one Spot Instance request `request_id` names and answer the state it is in
    /// afterwards, as of the instant `at` — `None` when this store holds no such request.
    ///
    /// The ONE writer of a Spot Instance request's cancellation, so a request cancelled on
    /// its own and one cancelled with the fleet that placed it
    /// ([`Self::cancel_spot_fleet_request`]) cannot end two different ways:
    ///
    /// * "`cancelled` - The request was canceled" (`SpotInstanceState`) is the state a request
    ///   in [`SPOT_REQUEST_STATE_OPEN`] or [`SPOT_REQUEST_STATE_ACTIVE`] enters; a request
    ///   that has already left both — one whose validity ran out, one already cancelled — has
    ///   nothing left to cancel and keeps the state it is in, which is what this answers;
    /// * the Spot request status code records HOW it ended:
    ///   [`SPOT_STATUS_CANCELED_BEFORE_FULFILLMENT`] when nothing had been launched for it,
    ///   [`SPOT_STATUS_INSTANCE_TERMINATED_BY_USER`] when the instance it launched is being
    ///   terminated with it, and [`SPOT_STATUS_REQUEST_CANCELED_AND_INSTANCE_RUNNING`] when
    ///   that instance is left running;
    /// * a cancelled request owes no fulfillment, which is what stops the next clock pass
    ///   launching an instance for it.
    ///
    /// `instance_terminated` says whether the CALLER is terminating the instance the request
    /// launched, which is the status code's third case; terminating it is the caller's own
    /// step ([`Self::begin_instance_termination`]), because "canceling a Spot Instance
    /// request does not terminate running Spot Instances associated with the request"
    /// (`CancelSpotInstanceRequests`) while a Spot Fleet cancellation may.
    pub fn cancel_spot_instance_request(
        &mut self,
        request_id: &str,
        instance_terminated: bool,
        at: SystemTime,
    ) -> Option<String> {
        let record = self.spot_instance_requests.get_mut(request_id)?;
        let state = record.view.state.as_deref().unwrap_or_default();
        if state != SPOT_REQUEST_STATE_OPEN && state != SPOT_REQUEST_STATE_ACTIVE {
            return Some(state.to_string());
        }
        let (code, message) = match (record.view.instance_id.is_some(), instance_terminated) {
            // Nothing was launched for this request, so it is cancelled before it was ever
            // fulfilled.
            (false, _) => (
                SPOT_STATUS_CANCELED_BEFORE_FULFILLMENT,
                SPOT_STATUS_MESSAGE_CANCELED_BEFORE_FULFILLMENT,
            ),
            (true, true) => (
                SPOT_STATUS_INSTANCE_TERMINATED_BY_USER,
                SPOT_STATUS_MESSAGE_INSTANCE_TERMINATED_BY_USER,
            ),
            (true, false) => (
                SPOT_STATUS_REQUEST_CANCELED_AND_INSTANCE_RUNNING,
                SPOT_STATUS_MESSAGE_REQUEST_CANCELED_AND_INSTANCE_RUNNING,
            ),
        };
        record.view.state = Some(SPOT_REQUEST_STATE_CANCELLED.to_string());
        record.view.status = Some(SpotInstanceStatus {
            code: Some(code.to_string()),
            message: Some(message.to_string()),
            update_time: Some(system_time_to_date_time(at)),
        });
        record.fulfillment = None;
        Some(SPOT_REQUEST_STATE_CANCELLED.to_string())
    }

    /// The product description a Spot request for `image_id` carries: which operating
    /// system the Spot Instance runs, read off the AMI the launch names when this account
    /// can see it.
    ///
    /// An AMI this store does not hold leaves the description at the Linux one, which is
    /// what the overwhelming majority of AMIs are and what EC2 reports for an AMI with no
    /// `Platform` of its own ("the value is `windows` for Windows AMIs; otherwise blank",
    /// `Image$Platform`).
    pub fn spot_product_description(&self, image_id: Option<&str>) -> &'static str {
        let windows = image_id
            .and_then(|image_id| self.images.get(image_id))
            .is_some_and(|image| {
                image
                    .platform
                    .as_deref()
                    .is_some_and(|platform| platform.eq_ignore_ascii_case(IMAGE_PLATFORM_WINDOWS))
            });
        if windows {
            PRODUCT_DESCRIPTION_WINDOWS
        } else {
            PRODUCT_DESCRIPTION_LINUX_UNIX
        }
    }
}
