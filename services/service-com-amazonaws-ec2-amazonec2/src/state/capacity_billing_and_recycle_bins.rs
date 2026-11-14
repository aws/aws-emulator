// SPDX-License-Identifier: CC0-1.0

//! The capacity-reservation billing requests, the instance event tag registrations, and the
//! volume recycle bin a deleted volume waits in.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// `CallerRole`'s `odcr-owner` member: the caller is asking as the Capacity Reservation
/// owner — "If you are the Capacity Reservation owner, specify this value to view requests
/// that you have initiated"
/// (`DescribeCapacityReservationBillingRequestsRequest$Role`).
pub const CALLER_ROLE_ODCR_OWNER: &str = "odcr-owner";

/// `CallerRole`'s `unused-reservation-billing-owner` member: the caller is asking as the
/// consumer account — "If you are the consumer account, specify this value to view requests
/// that have been sent to you" (that same member's doc).
pub const CALLER_ROLE_UNUSED_RESERVATION_BILLING_OWNER: &str = "unused-reservation-billing-owner";

/// Is `allowed` the `CallerRole` catalog — the value set the model declares for the `Role`
/// member a billing-request caller states its side of the assignment with?
///
/// The boundary's constraint walk reports a refused enum value as the value set it failed
/// rather than as the shape that declared it, so the catalog IS how that type is recognised —
/// a model fact, the same one codegen wrote into
/// [`crate::ops::types::CALLER_ROLE_VALUES`]. EC2 words a refused value per PARAMETER TYPE
/// ("not a valid value for RoleType"), which is why the wording is keyed to the type's
/// catalog and not to the operation that happened to carry it.
pub fn is_caller_role_catalog(allowed: &[String]) -> bool {
    allowed.len() == crate::ops::types::CALLER_ROLE_VALUES.len()
        && allowed
            .iter()
            .zip(crate::ops::types::CALLER_ROLE_VALUES)
            .all(|(held, member)| held == member)
}

/// How EC2 states a `Role` the `CallerRole` catalog does not hold: HTTP 400
/// `InvalidParameterValue` with the submitted value, the parameter type that refused it and
/// the values it accepts — "`<value>` is not a valid value for RoleType. Specify either
/// odcr-owner or unused-reservation-billing-owner." EC2 does not state a modelled refusal as
/// the generic validation clause; it names the type and what to send instead, and the only
/// part built from the request is the value echoed back.
///
/// The accepted values are read off the catalog itself, so a member the model adds later is
/// offered too rather than silently left out of the sentence.
pub fn caller_role_refusal_message(value: &str) -> String {
    format!(
        "{value} is not a valid value for RoleType. Specify either {}.",
        either_of(crate::ops::types::CALLER_ROLE_VALUES)
    )
}

/// The members of a catalog as a sentence offers them: `a`, `a or b`, `a, b or c`.
pub(super) fn either_of(values: &[&str]) -> String {
    match values.split_last() {
        None => String::new(),
        Some((last, [])) => (*last).to_string(),
        Some((last, leading)) => format!("{} or {last}", leading.join(", ")),
    }
}

/// `CapacityReservationBillingRequestStatus`'s `pending` member: a request that has been
/// sent and neither accepted nor rejected yet, which is the status a newly made billing
/// assignment request carries. The whole catalog is the generated
/// [`crate::ops::types::CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_VALUES`].
pub const CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_PENDING: &str = "pending";

/// `CapacityReservationBillingRequestStatus`'s `accepted` member: the consumer account the
/// request was sent to took the billing of the reservation's unused capacity on, which is
/// what `AcceptCapacityReservationBillingOwnership` makes of a `pending` request. From the
/// same catalog as
/// [`CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_PENDING`]
/// ([`crate::ops::types::CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_VALUES`]).
pub const CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_ACCEPTED: &str = "accepted";

/// How many Capacity Reservation billing requests this emulator stores before a writer must
/// refuse.
///
/// CHOSEN, like [`MAX_STORED_CAPACITY_BLOCKS`] and for the same reason: nothing in this
/// emulator reclaims a stored request — a rejected or revoked one is still reported — so the
/// store needs a ceiling that is not the process's memory. A count ceiling is no bound on the
/// caller-chosen bytes each record holds, so those are charged against [`MAX_STORED_BYTES`]
/// too ([`capacity_reservation_billing_request_payload_bytes`]).
pub const MAX_STORED_CAPACITY_RESERVATION_BILLING_REQUESTS: usize = 4096;

/// The fixed overhead one stored Capacity Reservation billing request costs whatever it
/// carries: the modelled record itself and the map entry holding it, as a CHOSEN constant in
/// the same spirit as [`VPC_ENDPOINT_CONNECTION_RECORD_BYTES`] — the point is that a request
/// carrying no status message and no reservation details is not free against
/// [`MAX_STORED_BYTES`], not that it is byte-exact.
pub const CAPACITY_RESERVATION_BILLING_REQUEST_RECORD_BYTES: usize = 512;

/// What one stored Capacity Reservation billing request is created with.
///
/// The spec of the resource, in the same shape [`VpcEndpointConnectionSpec`] has for a
/// connection: an operation that assigns the billing of a reservation's unused capacity fills
/// this in and
/// [`ComAmazonawsEc2Amazonec2State::insert_capacity_reservation_billing_request`] turns it
/// into the modelled [`CapacityReservationBillingRequest`] the describe reports, so the
/// stored key and the byte charge are decided in exactly one place.
#[derive(Debug, Clone)]
pub struct CapacityReservationBillingRequestSpec {
    /// The Capacity Reservation whose unused capacity the request is about
    /// (`CapacityReservationBillingRequest$CapacityReservationId`), which is also the key the
    /// request is stored under.
    pub capacity_reservation_id: String,
    /// "The ID of the Amazon Web Services account that initiated the request"
    /// (`CapacityReservationBillingRequest$RequestedBy`) — the Capacity Reservation owner.
    pub requested_by: String,
    /// "The ID of the Amazon Web Services account to which the request was sent"
    /// (`CapacityReservationBillingRequest$UnusedReservationBillingOwnerId`) — the consumer
    /// account.
    pub unused_reservation_billing_owner_id: String,
    /// When the request was last updated.
    pub last_update_time: SystemTime,
    /// The request's status, as a member of the modelled
    /// `CapacityReservationBillingRequestStatus` catalog. A `&'static str` so only a catalog
    /// member can ever be stored here: the word comes from this module's own constants or
    /// from [`crate::ops::types::CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_VALUES`], never
    /// from a request.
    pub status: &'static str,
    /// "Information about the status" (`CapacityReservationBillingRequest$StatusMessage`), or
    /// `None` for a status that carries none.
    pub status_message: Option<String>,
    /// "Information about the Capacity Reservation" the request is about
    /// (`CapacityReservationBillingRequest$CapacityReservationInfo`), or `None` for a request
    /// whose reservation this store knows nothing more about.
    pub capacity_reservation_info: Option<CapacityReservationInfo>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` Capacity Reservation billing requests fit inside the store's
    /// ceiling ([`MAX_STORED_CAPACITY_RESERVATION_BILLING_REQUESTS`]).
    ///
    /// A writer asks this BEFORE storing anything and refuses the request with EC2's own
    /// quota code when it answers `false`, the way
    /// [`Self::has_vpc_endpoint_connection_capacity`] gates its own store.
    /// [`Self::insert_capacity_reservation_billing_request`] asks it again for itself, so no
    /// path can get past the ceiling by forgetting to.
    pub fn has_capacity_reservation_billing_request_capacity(&self, count: usize) -> bool {
        self.capacity_reservation_billing_requests
            .len()
            .saturating_add(count)
            <= MAX_STORED_CAPACITY_RESERVATION_BILLING_REQUESTS
    }

    /// Store one request to assign the billing of a Capacity Reservation's unused capacity,
    /// and answer whether it was stored.
    ///
    /// The one place such a request enters this store, so every writer of one — and
    /// `DescribeCapacityReservationBillingRequests`, which reports what this wrote — agrees on
    /// the stored shape, the key and which members a request carries. A second request for the
    /// same `CapacityReservationId` REPLACES the first: one reservation's unused capacity is
    /// assigned to at most one account at a time. The bytes the record retains are charged
    /// here, so no path can store them uncharged.
    ///
    /// `false` means the store is at [`MAX_STORED_CAPACITY_RESERVATION_BILLING_REQUESTS`] and
    /// NOTHING was stored: the ceiling is enforced here as well as by
    /// [`Self::has_capacity_reservation_billing_request_capacity`], so a writer that forgets
    /// the gate cannot grow the store without bound either. A writer that sees `false`
    /// refuses the request with EC2's own quota code rather than reporting an assignment it
    /// does not hold. A replacement gives the displaced record's bytes back before charging
    /// the new one, so one reservation's request can be rewritten any number of times without
    /// inflating the byte budget.
    pub fn insert_capacity_reservation_billing_request(
        &mut self,
        spec: CapacityReservationBillingRequestSpec,
    ) -> bool {
        // A write that REPLACES the request of a reservation already stored adds no record,
        // so only a new key has to fit inside the ceiling — and the bytes the displaced
        // record held are given back before the new one is charged, so repeatedly replacing
        // one reservation's request cannot inflate the byte budget.
        let displaced = self
            .capacity_reservation_billing_requests
            .get(&spec.capacity_reservation_id)
            .map(capacity_reservation_billing_request_payload_bytes);
        if displaced.is_none() && !self.has_capacity_reservation_billing_request_capacity(1) {
            return false;
        }
        let request = CapacityReservationBillingRequest {
            capacity_reservation_id: Some(spec.capacity_reservation_id.clone()),
            requested_by: Some(spec.requested_by),
            unused_reservation_billing_owner_id: Some(spec.unused_reservation_billing_owner_id),
            last_update_time: Some(system_time_to_date_time(spec.last_update_time)),
            status: Some(spec.status.to_string()),
            status_message: spec.status_message,
            capacity_reservation_info: spec.capacity_reservation_info,
        };
        if let Some(bytes) = displaced {
            self.release_stored_bytes(bytes);
        }
        self.add_stored_bytes(capacity_reservation_billing_request_payload_bytes(&request));
        self.capacity_reservation_billing_requests
            .insert(spec.capacity_reservation_id, request);
        true
    }

    /// The billing assignment request `account` is the CONSUMER of — the one sent to it for
    /// `capacity_reservation_id` — or `None` when the store holds none.
    ///
    /// The one place a request is resolved from the side it was SENT to, so an operation the
    /// consumer account answers a request with and
    /// `DescribeCapacityReservationBillingRequests`' `unused-reservation-billing-owner` role
    /// select the same records: a request between two OTHER accounts is not one this account
    /// can see, let alone answer.
    pub fn capacity_reservation_billing_request_sent_to(
        &self,
        capacity_reservation_id: &str,
        account: &str,
    ) -> Option<&CapacityReservationBillingRequest> {
        self.capacity_reservation_billing_requests
            .get(capacity_reservation_id)
            .filter(|request| {
                request.unused_reservation_billing_owner_id.as_deref() == Some(account)
            })
    }

    /// Move the billing assignment request stored for `capacity_reservation_id` to `status`,
    /// as of `now`, and answer whether it was moved.
    ///
    /// The one place a stored request's status is answered, so every operation that answers
    /// one — and the describe that reports it — agrees on what an answered request carries:
    /// the status, the message it is reported with, and `LastUpdateTime` as the instant the
    /// answer was given ("When the request was last updated"). `status` is a `&'static str`,
    /// so only a member of the modelled `CapacityReservationBillingRequestStatus` catalog can
    /// be written. The bytes the record holds are re-charged here, so a status message that
    /// grows or shrinks the record cannot drift from [`MAX_STORED_BYTES`].
    ///
    /// `false` means the store holds no request for that reservation and NOTHING was written.
    pub fn answer_capacity_reservation_billing_request(
        &mut self,
        capacity_reservation_id: &str,
        status: &'static str,
        status_message: Option<String>,
        now: SystemTime,
    ) -> bool {
        let Some((held, charged)) = self
            .capacity_reservation_billing_requests
            .get_mut(capacity_reservation_id)
            .map(|request| {
                let held = capacity_reservation_billing_request_payload_bytes(request);
                request.status = Some(status.to_string());
                request.status_message = status_message;
                request.last_update_time = Some(system_time_to_date_time(now));
                (
                    held,
                    capacity_reservation_billing_request_payload_bytes(request),
                )
            })
        else {
            return false;
        };
        self.release_stored_bytes(held);
        self.add_stored_bytes(charged);
        true
    }
}

/// What one stored Capacity Reservation billing request costs the store: the fixed overhead
/// of the record ([`CAPACITY_RESERVATION_BILLING_REQUEST_RECORD_BYTES`]) plus the
/// caller-chosen bytes it holds — the reservation id it is keyed by, its status message, and
/// the instance type and zone of the reservation it reports. The two account ids are bounded
/// by the model's own `@length(12, 12)` on `AccountID`, and the status and the instant are
/// derived by this module, so those are covered by the fixed part.
///
/// The ONE definition of that charge, the way [`vpc_endpoint_connection_payload_bytes`] is
/// for a connection: a writer charges exactly it, and a removal path must give back the same
/// number.
pub fn capacity_reservation_billing_request_payload_bytes(
    request: &CapacityReservationBillingRequest,
) -> usize {
    let text = |value: Option<&String>| -> usize { value.map(String::len).unwrap_or_default() };
    let info_bytes = request
        .capacity_reservation_info
        .as_ref()
        .map(|info| {
            text(info.instance_type.as_ref())
                + text(info.availability_zone.as_ref())
                + text(info.availability_zone_id.as_ref())
                + text(info.tenancy.as_ref())
        })
        .unwrap_or_default();
    CAPACITY_RESERVATION_BILLING_REQUEST_RECORD_BYTES
        .saturating_add(text(request.capacity_reservation_id.as_ref()))
        .saturating_add(text(request.status_message.as_ref()))
        .saturating_add(info_bytes)
}

// =============================================================================
// Amazon S3 storage locations a request names
// =============================================================================

/// The code EC2 answers when it cannot read the Amazon S3 location a request named — the
/// bucket is not one it can reach at all.
pub const INACCESSIBLE_STORAGE_LOCATION: &str = "InaccessibleStorageLocation";

/// The shortest name a general-purpose Amazon S3 bucket may have: "Bucket names must be
/// between 3 (min) and 63 (max) characters long" (Amazon S3 User Guide, "General purpose
/// bucket naming rules").
pub const MIN_S3_BUCKET_NAME_CHARS: usize = 3;

/// The longest name a general-purpose Amazon S3 bucket may have (see
/// [`MIN_S3_BUCKET_NAME_CHARS`]).
pub const MAX_S3_BUCKET_NAME_CHARS: usize = 63;

/// The prefixes a general-purpose bucket name must not start with: "Bucket names must not
/// start with the prefix `xn--`. … must not start with the prefix `sthree-`. … must not
/// start with the prefix `amzn-s3-demo-`" (Amazon S3 User Guide, "General purpose bucket
/// naming rules").
pub const S3_BUCKET_NAME_RESERVED_PREFIXES: [&str; 3] = ["xn--", "sthree-", "amzn-s3-demo-"];

/// The suffixes a general-purpose bucket name must not end with: "Bucket names must not end
/// with the suffix `-s3alias`. … must not end with the suffix `--ol-s3`. … must not end with
/// the suffix `.mrap`. … must not end with the suffix `--x-s3`. … must not end with the
/// suffix `--table-s3`" (Amazon S3 User Guide, "General purpose bucket naming rules").
pub const S3_BUCKET_NAME_RESERVED_SUFFIXES: [&str; 5] =
    ["-s3alias", "--ol-s3", ".mrap", "--x-s3", "--table-s3"];

/// Is `name` a legal general-purpose Amazon S3 bucket name?
///
/// Every rule below is one sentence of the Amazon S3 User Guide's "General purpose bucket
/// naming rules": the length ([`MIN_S3_BUCKET_NAME_CHARS`]..=[`MAX_S3_BUCKET_NAME_CHARS`]),
/// "can consist only of lowercase letters, numbers, dots (.), and hyphens (-)", "must begin
/// and end with a letter or number", "must not contain two adjacent periods", "must not be
/// formatted as an IP address (for example, 192.168.5.4)", and the reserved prefixes and
/// suffixes above. A value that breaks any of them names no bucket at all, so no service can
/// read an object out of it.
pub fn s3_bucket_name_is_well_formed(name: &str) -> bool {
    // The character set FIRST, so the length rule below counts characters of a name that
    // can only be ASCII — "can consist only of lowercase letters, numbers, dots (.), and
    // hyphens (-)" — and a byte count is that character count.
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.')
    {
        return false;
    }
    if !(MIN_S3_BUCKET_NAME_CHARS..=MAX_S3_BUCKET_NAME_CHARS).contains(&name.len()) {
        return false;
    }
    let begins_and_ends_alphanumeric = name
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && name
            .bytes()
            .next_back()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    if !begins_and_ends_alphanumeric {
        return false;
    }
    if name.contains("..") {
        return false;
    }
    // "Must not be formatted as an IP address": four dot-separated decimal octets.
    if name.parse::<::std::net::Ipv4Addr>().is_ok() {
        return false;
    }
    if S3_BUCKET_NAME_RESERVED_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        return false;
    }
    !S3_BUCKET_NAME_RESERVED_SUFFIXES
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

/// Refuse a request whose Amazon S3 storage location this service cannot read: HTTP 400
/// `InaccessibleStorageLocation`.
pub fn inaccessible_storage_location<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(INACCESSIBLE_STORAGE_LOCATION, 400, message)
}

/// The most characters an Amazon S3 KEY PREFIX a request names may carry: the published
/// limit on the length of an Amazon S3 object key, which a prefix is the leading part of
/// (Amazon S3 User Guide, "Creating object key names" — "the name for a key is a sequence of
/// Unicode characters whose UTF-8 encoding is at most 1,024 bytes long").
///
/// The ONE bound every operation of this crate that takes an S3 key prefix holds it to: no
/// member of that kind carries a modelled `@length`, so this is the only place that limit is
/// kept, and it is also what bounds the caller-chosen bytes one stored record retains for it.
pub const MAX_S3_KEY_PREFIX_CHARS: usize = 1024;

/// Refuse a `Bucket` that is outside the Amazon S3 general purpose bucket naming rules
/// ([`s3_bucket_name_is_well_formed`]): HTTP 400 [`INVALID_REQUEST`], `The S3 bucket name
/// contains invalid characters.`
///
/// Such a value names no bucket at all, so it is a malformed PARAMETER rather than a location
/// that could not be reached — a permanent fault of the request, not something to retry or
/// get access to. The message is fixed text with no variable part: the parameter is reported
/// as unusable, and nothing is said about a bucket, because such a name reaches none. The ONE
/// answer the operations that take an AMI storage location share, so a bad bucket name reads
/// the same whichever of them a caller sent it to.
pub fn invalid_s3_bucket_name<E>() -> Scaffolded<E> {
    invalid_request("The S3 bucket name contains invalid characters.")
}

/// The bucket and the key of a storage location a request named.
///
/// `member` names the request member the location came from, so the message says which one
/// was wrong. `StorageLocation` declares neither member required, so a location that names
/// no bucket, or names a bucket and no key, names no object at all and is EC2's
/// `MissingParameter` — a fact of the request, answered before anything is read.
pub fn storage_location_object<'a, E>(
    location: &'a StorageLocation,
    member: &str,
) -> Result<(&'a str, &'a str), Scaffolded<E>> {
    let bucket = location
        .bucket
        .as_deref()
        .filter(|bucket| !bucket.is_empty())
        .ok_or_else(|| missing_parameter(format!("{member} must name an S3 bucket")))?;
    let key = location
        .key
        .as_deref()
        .filter(|key| !key.is_empty())
        .ok_or_else(|| missing_parameter(format!("{member} must name an S3 object key")))?;
    Ok((bucket, key))
}

/// Refuse a request whose work was to read the Amazon S3 object at `bucket`/`key`, which
/// this service could not access: HTTP 400 [`INACCESSIBLE_STORAGE_LOCATION`].
///
/// One answer for every reason a location cannot be read — the bucket is not a bucket name
/// at all, holds no such object, or lies outside the Region the call was made in — because
/// the caller is told the location that failed and what to check about it, which is what EC2
/// answers with for this error class.
pub fn inaccessible_s3_resource<E>(bucket: &str, key: &str) -> Scaffolded<E> {
    inaccessible_storage_location(format!(
        "Error accessing resource in S3. Check permissions, naming, and ensure the bucket \
         is within the same region as the API endpoint. Bucket: {bucket}, Key: {key}."
    ))
}

// =============================================================================
// Amazon FPGA Images (AFIs)
// =============================================================================

/// An AFI whose creation is still running: `FpgaImageStateCode`'s `pending`
/// (`crate::ops::types::FPGA_IMAGE_STATE_CODE_VALUES` is the generated catalog).
pub const FPGA_IMAGE_STATE_PENDING: &str = "pending";

/// An AFI whose creation did not produce a bitstream: `FpgaImageStateCode`'s `failed`.
pub const FPGA_IMAGE_STATE_FAILED: &str = "failed";

/// The id prefix of an Amazon FPGA Image: `afi-`.
pub const FPGA_IMAGE_ID_PREFIX: &str = "afi";

/// The one resource type `CreateFpgaImage`'s `TagSpecifications` can name: "The tags to
/// apply to the FPGA image during creation"
/// (`CreateFpgaImageRequest$TagSpecifications`), which is the `fpga-image` member of
/// `ResourceType`.
///
/// It is also the term EC2 names this resource by when a request could not resolve one
/// ([`invalid_fpga_image_id_not_found`]), so the spelling of the resource type lives in one
/// place.
pub const FPGA_IMAGE_TAGGABLE_RESOURCE: &str = "fpga-image";

/// The resource types a restore of an AMI from Amazon S3 may tag: "You can tag the AMI, the
/// snapshots, or both. To tag the AMI, the value for `ResourceType` must be `image`. To tag
/// the snapshots, the value for `ResourceType` must be `snapshot`"
/// (`CreateRestoreImageTaskRequest$TagSpecifications`) — the `image` and `snapshot` members
/// of `ResourceType`, each named once in this crate ([`super::IMAGE_TAGGABLE_RESOURCE`] and
/// [`super::SNAPSHOT_TAGGABLE_RESOURCE`]).
pub const IMAGE_TAGGABLE_RESOURCES: [&str; 2] = [
    super::IMAGE_TAGGABLE_RESOURCE,
    super::SNAPSHOT_TAGGABLE_RESOURCE,
];

/// The id prefix of the global identifier of an Amazon FPGA Image: `agfi-`
/// ("The global FPGA image identifier (AGFI ID)", `CreateFpgaImageResult$FpgaImageGlobalId`).
pub const FPGA_IMAGE_GLOBAL_ID_PREFIX: &str = "agfi";

/// The code EC2 answers for something that is not an AFI id at all.
pub const INVALID_FPGA_IMAGE_ID_MALFORMED: &str = "InvalidFpgaImageID.Malformed";

/// The code EC2 answers for an AFI id the account holds no FPGA image for.
pub const INVALID_FPGA_IMAGE_ID_NOT_FOUND: &str = "InvalidFpgaImageID.NotFound";

/// EC2's own code for a request that would take the account past a resource limit this
/// service answers no resource-specific code for — "You have exceeded an Amazon EC2
/// resource limit" (EC2 API error codes, `ResourceLimitExceeded`).
pub const RESOURCE_LIMIT_EXCEEDED: &str = "ResourceLimitExceeded";

/// How many Amazon FPGA Images this emulator stores before a call that mints one is
/// refused.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: the store lives in this
/// process's memory, so the number of AFIs a caller can create has to be finite. It is well
/// above the published per-Region AFI quota (10 by default, which is adjustable), so a test
/// that creates more than a real account's default quota is not refused for a reason this
/// emulator cannot have.
pub const MAX_FPGA_IMAGES: usize = 1_000;

/// Refuse a request that would take the store past a ceiling with no resource-specific code
/// of its own: HTTP 400 [`RESOURCE_LIMIT_EXCEEDED`].
pub fn resource_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(RESOURCE_LIMIT_EXCEEDED, 400, message)
}

/// Is `id` shaped like an AFI id EC2 could have ISSUED?
///
/// `afi-` followed by 8 or 17 lowercase hexadecimal digits — the short and the long id form,
/// the same two forms [`volume_id_is_well_formed`] and [`image_id_is_well_formed`] accept,
/// read by the one [`prefixed_id_is_well_formed`] every resource of this module asks — of
/// which not every digit is `0` ([`id_is_not_the_null_id`]). The null id is the placeholder
/// every client reaches for and the mint counts from one
/// ([`ComAmazonawsEc2Amazonec2State::next_id`]), so no AFI can ever hold it: the recorded
/// refusal of an all-zero AFI id is `InvalidFpgaImageID.Malformed`, not the `NotFound` a
/// well-formed id nothing is stored under is answered with. That clause is a fact of THIS id
/// kind, the way it is of an instance id ([`instance_id_is_well_formed`]) and a Capacity
/// Reservation Fleet id, and not of every EC2 id — a network interface id answers `NotFound`
/// for the same suffix — so it is read here rather than in [`prefixed_id_is_well_formed`].
///
/// EC2 answers `InvalidFpgaImageID.Malformed` for anything else, BEFORE it looks the id up.
pub fn fpga_image_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(FPGA_IMAGE_ID_PREFIX, id)
        && id_is_not_the_null_id(FPGA_IMAGE_ID_PREFIX, id)
}

/// Refuse a request that named an AFI id the account holds no FPGA image for: HTTP 400
/// `InvalidFpgaImageID.NotFound`.
///
/// EC2 words a resource it could not resolve as "The `<resource-type>` ID '`<id>`' does not
/// exist", naming the resource by the `ResourceType` member that spells it —
/// [`FPGA_IMAGE_TAGGABLE_RESOURCE`] for an AFI, the same way this crate's refusals for a
/// verified-access instance, a route server and a transit-gateway attachment read. Only the
/// id is variable, and it is the one the REQUEST carried.
pub fn invalid_fpga_image_id_not_found<E>(fpga_image_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_FPGA_IMAGE_ID_NOT_FOUND,
        400,
        format!(
            "The {FPGA_IMAGE_TAGGABLE_RESOURCE} ID '{fpga_image_id}' does not exist"
        ),
    )
}

/// Refuse an AFI id the request named that is not an AFI id at all: HTTP 400
/// `InvalidFpgaImageID.Malformed`, which EC2 answers BEFORE it looks the id up.
///
/// The pair of this and [`invalid_fpga_image_id_not_found`] is the ONE reading of the
/// malformed-before-not-found order for an AFI; they are two functions rather than one
/// `require_known_…` because every operation that takes an AFI id answers its `DryRun`
/// probe between them.
///
/// The AFI refusal words itself `Image ID '<id>' is invalid.` rather than with the
/// [`invalid_id_message`] template the other id kinds of this service share: that is what the
/// recorded refusals of an unreadable AFI id show, on the copy path
/// (`crate::ops::copy_fpga_image`) and on the delete path alike, so it is the wording of the
/// FPGA-image id kind and lives here, where every operation resolving an AFI reads it. Only
/// the id is variable, and it is the one the REQUEST carried, bounded by
/// [`echoed_resource_id`] the way every refusal that echoes a caller-chosen id is.
pub fn invalid_fpga_image_id_malformed<E>(fpga_image_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_FPGA_IMAGE_ID_MALFORMED,
        400,
        format!("Image ID '{}' is invalid.", echoed_resource_id(fpga_image_id)),
    )
}

/// One stored Amazon FPGA Image: the modelled [`FpgaImage`] every operation that reports one
/// answers with, its load permissions, and the move the clock owes its creation.
///
/// The view holds no `State`: an AFI's state is a fact of how far its creation has got, so
/// [`FpgaImageRecord::view_at`] derives it from the stored due instant rather than letting
/// two writers store two different words for it — the same way [`VolumeRecord`] treats a
/// volume's state.
///
/// Minting an AFI is asynchronous — "to verify that the AFI was successfully created and is
/// ready for use, check the output logs" (`CreateFpgaImage`) — so the call that mints one
/// hands back a `pending` AFI and the outcome is reported in that AFI's own state. This
/// emulator holds no Amazon S3 objects, so no bitstream is ever built from a design
/// checkpoint and no AFI of this store carries one: the move therefore ends in the modelled
/// `failed` state carrying [`FpgaImageRecord::failure_message`], which says so, rather than
/// in an `available` AFI nothing built.
#[derive(Debug, Clone)]
pub struct FpgaImageRecord {
    /// The AFI as the model shapes it, with `State` left for [`FpgaImageRecord::view_at`].
    pub view: FpgaImage,
    /// "The load permissions" (`FpgaImageAttribute$LoadPermissions`) of this AFI: which
    /// accounts and groups may load it onto an FPGA. `ModifyFpgaImageAttribute` adds and
    /// removes them and `ResetFpgaImageAttribute` resets them to the default, which is none
    /// — an AFI starts loadable by its owner only.
    pub load_permissions: Vec<LoadPermission>,
    /// The instant from which this AFI's `pending` -> `failed` move is due. Monotonic
    /// ([`Instant`]) for the same reason [`VolumeRecord::available_at`] is.
    pub resolved_at: Instant,
    /// Why the creation failed, reported as `FpgaImageState$Message` once the move above is
    /// due. Built from the request's own storage location, never from a recording.
    pub failure_message: String,
    /// The caller-chosen bytes this record was charged against [`MAX_STORED_BYTES`] when it
    /// was stored ([`fpga_image_payload_bytes`]), so the removal path gives back exactly
    /// what the create charged.
    pub payload_bytes: usize,
}

impl FpgaImageRecord {
    /// This AFI as of `now`: the stored view with the `State` the clock has reached.
    pub fn view_at(&self, now: Instant) -> FpgaImage {
        let mut view = self.view.clone();
        view.state = Some(self.state_at(now));
        view
    }

    /// The AFI's `FpgaImageState` at `now`.
    pub fn state_at(&self, now: Instant) -> FpgaImageState {
        if now < self.resolved_at {
            FpgaImageState {
                code: Some(FPGA_IMAGE_STATE_PENDING.to_string()),
                message: None,
            }
        } else {
            FpgaImageState {
                code: Some(FPGA_IMAGE_STATE_FAILED.to_string()),
                message: Some(self.failure_message.clone()),
            }
        }
    }
}

/// The caller-chosen bytes one stored AFI holds: its tags, the name and description the
/// request gave it, the storage locations it named and the `ClientToken` the token map keeps
/// for it. Everything else on the record — the ids, the account, the states and the
/// timestamps — is derived by this module, so it is fixed-size.
///
/// The ONE definition of that charge, the way [`volume_payload_bytes`] is for a volume: a
/// writer charges exactly it, and a removal path gives back the same number.
pub fn fpga_image_payload_bytes(
    tags: &[Tag],
    name: Option<&str>,
    description: Option<&str>,
    locations: &[&str],
    client_token: Option<&str>,
) -> usize {
    tag_list_bytes(tags)
        .saturating_add(name.map_or(0, str::len))
        .saturating_add(description.map_or(0, str::len))
        .saturating_add(locations.iter().map(|value| value.len()).sum())
        .saturating_add(client_token.map_or(0, str::len))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Take the AFI `fpga_image_id` out of the store, and answer the record as it was last
    /// stored; `None` when the store holds no such AFI.
    ///
    /// The bytes the record was charged are given back and any `ClientToken` that named it
    /// is dropped, so a token can never name an AFI that is gone — the same contract
    /// [`Self::remove_launch_template`] keeps for a launch template.
    pub fn remove_fpga_image(&mut self, fpga_image_id: &str) -> Option<FpgaImageRecord> {
        let record = self.fpga_images.remove(fpga_image_id)?;
        self.fpga_image_client_tokens
            .retain(|_, minted| minted != fpga_image_id);
        self.release_stored_bytes(record.payload_bytes);
        Some(record)
    }
}

// =============================================================================
// AMI watermarks
// =============================================================================

/// The characters a `WatermarkName` may carry beside letters and digits: "Constraints: 3-128
/// alphanumeric characters, parentheses (()), square brackets ([]), spaces ( ), periods (.),
/// slashes (/), dashes (-), single quotes ('), at-signs (@), or underscores(_)"
/// (`AttachImageWatermarkRequest$WatermarkName`). The LENGTH of that sentence is modelled
/// (`@length(min: 3, max: 128)`) and refused before a handler runs; the character set is
/// not, so a handler is the only place it can be enforced.
pub const WATERMARK_NAME_PUNCTUATION: &str = "()[] ./-'@_";

/// How many watermarks one AMI carries before an attach is refused.
///
/// CHOSEN: AWS publishes no quota for watermarks per AMI, and each attach retains the
/// caller's watermark name for the process's lifetime, so the list needs a ceiling that is
/// not the process's memory. The bytes each watermark retains are charged against
/// [`MAX_STORED_BYTES`] as well.
pub const MAX_WATERMARKS_PER_IMAGE: usize = 50;

/// Is `name` a well-formed `WatermarkName` — does it carry only the characters
/// [`WATERMARK_NAME_PUNCTUATION`]'s sentence allows?
pub fn watermark_name_is_well_formed(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || WATERMARK_NAME_PUNCTUATION.contains(c))
}

/// The `WatermarkKey` a watermark named `watermark_name` attached by `account` carries: "The
/// watermark identifier, in `accountId:watermarkName` format (for example,
/// `123456789012:approvedAmi`). The `accountId` portion is the Amazon Web Services account
/// ID of the watermark creator. The `watermarkName` portion is customer-provided"
/// (`ImageWatermark$WatermarkKey`).
pub fn image_watermark_key(account: &str, watermark_name: &str) -> String {
    format!("{account}:{watermark_name}")
}

// =============================================================================
// The Recycle Bin
// =============================================================================

/// A volume that has been deleted: `VolumeState`'s `deleted`, which is what a volume held in
/// the Recycle Bin reports — it has been deleted, and is kept only until its retention
/// period runs out.
pub const VOLUME_STATE_DELETED: &str = "deleted";

/// The page sizes a report of the Recycle Bin's volumes accepts: "Valid range: 5 - 500"
/// (`ListVolumesInRecycleBinRequest$MaxResults`). A request that names none gets the top of
/// this range, so the `NextToken` path behaves the same whether or not a page size was asked
/// for.
pub const RECYCLE_BIN_PAGE_SIZE_RANGE: (i32, i32) = (5, 500);

/// The bytes one entry of the Recycle Bin holds.
///
/// The ONE definition of that charge, the way [`volume_payload_bytes`] is for a volume: the
/// fixed [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs — so an entry carrying
/// no optional text is NOT free against [`MAX_STORED_BYTES`] — plus the text the entry
/// carries over from the volume it was derived from, which is text a request chose.
pub fn volume_recycle_bin_entry_payload_bytes(entry: &VolumeRecycleBinInfo) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(entry.volume_type.as_deref().map_or(0, str::len))
        .saturating_add(entry.outpost_arn.as_deref().map_or(0, str::len))
        .saturating_add(entry.snapshot_id.as_deref().map_or(0, str::len))
        .saturating_add(entry.source_volume_id.as_deref().map_or(0, str::len))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another entry fits inside the Recycle Bin's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]).
    ///
    /// The same contract as [`Self::has_transit_gateway_attachment_capacity`]: a caller asks
    /// this BEFORE moving anything and refuses the request with EC2's own quota code when it
    /// answers `false`, and [`Self::move_volume_to_recycle_bin`] asks it again for itself, so
    /// no path can get past the ceiling by forgetting to. The ceiling is needed because
    /// nothing in this emulator reclaims an entry of the bin, so without it a sequence of
    /// create-then-delete requests would grow memory without bound.
    pub fn has_volume_recycle_bin_capacity(&self) -> bool {
        self.volumes_in_recycle_bin.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Move the stored volume `volume_id` into the Recycle Bin, where it stays for
    /// `retention`, and answer the entry a report of the bin now holds for it.
    ///
    /// The entry is DERIVED from the stored
    /// volume rather than taken from a request, so a volume's size, type, zone and
    /// provisioning cannot read differently inside the bin from outside it; `retention` is
    /// the retention period of the Recycle Bin rule that covered the volume, which is a fact
    /// of that rule (Amazon Recycle Bin, "Retention rules") and so is given by the caller.
    ///
    /// `None` when the store holds no such volume, when the bin is at
    /// [`MAX_STORED_RECORDS_PER_STORE`], or when `now + retention` is not a representable
    /// instant. In each case NOTHING was removed and nothing stored — every fallible step is
    /// taken before the first mutation — so a refused move leaves the volume where it was
    /// instead of losing it between the two stores.
    ///
    /// `now` is the instant of THIS call — the wall-clock reading of the moment the volume is
    /// being moved, which every caller takes as `SystemTime::now()`. It is a parameter rather
    /// than read here so the reported `RecycleBinEnterTime`/`RecycleBinExitTime` and the rest
    /// of the calling operation's timestamps are one reading of one moment; the monotonic
    /// deadline the permanent deletion is enforced against is the SAME moment read off the
    /// clock that cannot jump, so the two never disagree by more than the two reads.
    ///
    /// The byte accounting is symmetric, the way [`Self::remove_fpga_image`] is: the bytes
    /// the create charged for the volume are given back as it leaves the store, and the
    /// entry that takes its place is charged for itself
    /// ([`volume_recycle_bin_entry_payload_bytes`]), so a repeated create-then-delete
    /// sequence advances the byte budget rather than trading a charged volume for a free
    /// entry.
    pub fn move_volume_to_recycle_bin(
        &mut self,
        volume_id: &str,
        now: SystemTime,
        retention: Duration,
    ) -> Option<VolumeRecycleBinInfo> {
        if !self.has_volume_recycle_bin_capacity() {
            return None;
        }
        // "The date and time when the volume is to be permanently deleted from the Recycle
        // Bin" is `now` plus the retention period the rule carries, and `SystemTime + Duration`
        // PANICS when that instant is not representable. The retention period comes from the
        // caller, so the sum is asked for with `checked_add` and the move is refused rather
        // than unwinding — and it is asked BEFORE the volume is removed.
        let exit_at = now.checked_add(retention)?;
        // The same instant on the MONOTONIC clock, which is what the permanent deletion the
        // clock owes is compared against ([`Self::materialize_due_transitions`]), so a
        // wall-clock adjustment can neither keep an expired entry nor expire a fresh one.
        // Fallible, and asked BEFORE the volume is removed, for the same reason as above.
        let exit_due_at = Instant::now().checked_add(retention)?;
        let record = self.volumes.remove(volume_id)?;
        let client_token = self
            .volume_client_tokens
            .iter()
            .find(|(_, minted)| minted.as_str() == volume_id)
            .map(|(token, _)| token.clone());
        if let Some(token) = &client_token {
            self.volume_client_tokens.remove(token);
        }
        // The volume is kept whole, so the restore gives back the SAME record: the entry
        // below is DERIVED from this view rather than taking it over.
        let view = &record.view;
        // Give back exactly what `CreateVolume` charged for this volume, the way
        // [`Self::remove_fpga_image`] does for an FPGA image: the byte budget stays the sum of
        // what the store actually holds, and the entry stored below is charged for itself.
        self.release_stored_bytes(volume_payload_bytes(
            view.tags.as_deref().unwrap_or_default(),
            view.kms_key_id.as_deref(),
            client_token.as_deref(),
        ));
        let entry = VolumeRecycleBinInfo {
            volume_id: view.volume_id.clone(),
            volume_type: view.volume_type.clone(),
            // "The state of the volume" (`VolumeRecycleBinInfo$State`): a volume in the
            // Recycle Bin has been deleted and is kept only until its retention period runs
            // out, which is `VolumeState`'s `deleted`.
            state: Some(VOLUME_STATE_DELETED.to_string()),
            size: view.size,
            iops: view.iops,
            throughput: view.throughput,
            outpost_arn: view.outpost_arn.clone(),
            availability_zone: view.availability_zone.clone(),
            availability_zone_id: view.availability_zone_id.clone(),
            source_volume_id: view.source_volume_id.clone(),
            snapshot_id: view.snapshot_id.clone(),
            operator: view.operator.clone(),
            create_time: view.create_time,
            // "The date and time when the volume entered the Recycle Bin" — now — and "the
            // date and time when the volume is to be permanently deleted from the Recycle
            // Bin", which is when the retention period that covered it runs out.
            recycle_bin_enter_time: Some(system_time_to_date_time(now)),
            recycle_bin_exit_time: Some(system_time_to_date_time(exit_at)),
        };
        self.add_stored_bytes(volume_recycle_bin_entry_payload_bytes(&entry));
        self.volumes_in_recycle_bin.insert(
            volume_id.to_string(),
            RecycledVolumeRecord {
                entry: entry.clone(),
                volume: record,
                client_token,
                exit_due_at,
            },
        );
        Some(entry)
    }

    /// Take the volume the Recycle Bin holds for `volume_id` back into the account's
    /// volumes, and answer whether the bin held one.
    ///
    /// The MOVE back: the record the bin kept is the record the volume store gets, so the
    /// volume comes back with the id, ARN, create time, provisioning, tags and attachment
    /// state it had, and the `ClientToken` that minted it names it again — a repeat of that
    /// create answers with this volume rather than minting a second one. The byte accounting
    /// is the mirror of [`Self::move_volume_to_recycle_bin`]: the entry gives its bytes back
    /// and the volume is charged what the create charged for it, so a volume that goes into
    /// the bin and comes back leaves the budget where it started.
    ///
    /// `false` when the bin holds no entry for `volume_id`, and then NOTHING was written.
    pub fn restore_recycled_volume(&mut self, volume_id: &str) -> bool {
        let Some(record) = self.volumes_in_recycle_bin.remove(volume_id) else {
            return false;
        };
        self.release_stored_bytes(volume_recycle_bin_entry_payload_bytes(&record.entry));
        self.charge_and_store_restored_volume(volume_id, record);
        true
    }

    /// Put one restored volume back in the volume store, charged exactly what
    /// `CreateVolume` charged for it — the one writer of that charge on the restore path,
    /// so the move out of the bin and the move back cannot disagree about it.
    pub(super) fn charge_and_store_restored_volume(&mut self, volume_id: &str, record: RecycledVolumeRecord) {
        let RecycledVolumeRecord {
            volume,
            client_token,
            ..
        } = record;
        self.add_stored_bytes(volume_payload_bytes(
            volume.view.tags.as_deref().unwrap_or_default(),
            volume.view.kms_key_id.as_deref(),
            client_token.as_deref(),
        ));
        if let Some(token) = client_token {
            self.volume_client_tokens
                .insert(token, volume_id.to_string());
        }
        self.volumes.insert(volume_id.to_string(), volume);
    }

    /// Apply the permanent deletions the clock owes the Recycle Bin: an entry whose
    /// retention period has run out is deleted for good — "the date and time when the volume
    /// is to be permanently deleted from the Recycle Bin"
    /// (`VolumeRecycleBinInfo$RecycleBinExitTime`) — whether or not anybody called, so
    /// neither a report of the bin nor a restore can see it afterwards, and the bytes the
    /// entry was charged go back to the budget.
    ///
    /// Reached only through [`Self::materialize_due_transitions`], so a reporting operation
    /// materialises nothing but what the clock already made due.
    pub(super) fn delete_due_recycled_volumes(&mut self, now: Instant) {
        let expired: Vec<String> = self
            .volumes_in_recycle_bin
            .iter()
            .filter(|(_, record)| now >= record.exit_due_at)
            .map(|(volume_id, _)| volume_id.clone())
            .collect();
        for volume_id in expired {
            if let Some(record) = self.volumes_in_recycle_bin.remove(&volume_id) {
                self.release_stored_bytes(volume_recycle_bin_entry_payload_bytes(&record.entry));
            }
        }
    }
}

/// One volume the Recycle Bin holds: the modelled entry a report of the bin answers with,
/// beside the stored volume itself, so a restore gives back the SAME resource rather than
/// one rebuilt from the entry's members — the entry carries neither the volume's tags nor
/// its encryption, and a restored volume keeps both.
#[derive(Debug, Clone)]
pub struct RecycledVolumeRecord {
    /// What `ListVolumesInRecycleBin` reports for this volume, derived from the volume when
    /// it entered the bin ([`ComAmazonawsEc2Amazonec2State::move_volume_to_recycle_bin`] is
    /// the one writer).
    pub entry: VolumeRecycleBinInfo,
    /// The volume exactly as the volume store held it, so
    /// [`ComAmazonawsEc2Amazonec2State::restore_recycled_volume`] gives back the same
    /// resource.
    pub volume: VolumeRecord,
    /// The `ClientToken` that minted the volume, if one did, so a restore makes that token
    /// name this volume again instead of leaving it able to mint a second one.
    pub client_token: Option<String>,
    /// The instant from which this entry's permanent deletion is due — the monotonic
    /// reading of its `RecycleBinExitTime`, never reported, so it is the clock's and not a
    /// member of the entry two writers could disagree about.
    pub exit_due_at: Instant,
}

impl RecycledVolumeRecord {
    /// How much of this entry's retention period is left at `now`, which is zero once the
    /// permanent deletion is due.
    pub fn retention_left(&self, now: Instant) -> Duration {
        self.exit_due_at.saturating_duration_since(now)
    }
}
