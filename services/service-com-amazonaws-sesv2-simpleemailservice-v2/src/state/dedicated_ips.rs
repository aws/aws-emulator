// SPDX-License-Identifier: CC0-1.0

//! The dedicated-IP plane of this service's state: the pools the account holds, the
//! dedicated addresses in them, and the third-party blocklist listings observed for
//! those addresses.
//!
//! Split out of the state module root so one resource plane's records live together;
//! every item is re-exported from [`crate::state`], so a handler still reaches it as
//! `crate::state::…`.

use std::time::SystemTime;

use super::SCALING_MODE_STANDARD;

/// One dedicated IP pool: "a pool can include one or more dedicated IP addresses that
/// are associated with your Amazon Web Services account"
/// (`CreateDedicatedIpPool`'s own documentation).
///
/// The addresses a pool holds are not kept here: no operation of this emulator
/// allocates a dedicated IP address, so a pool holds the attributes its create and its
/// scaling write and nothing invented.
#[derive(Debug, Clone)]
pub struct DedicatedIpPoolRecord {
    /// `DedicatedIpPool$PoolName`, as the request that created the pool spelled it —
    /// also the key this record is held under.
    pub pool_name: String,
    /// `DedicatedIpPool$ScalingMode`, a `ScalingMode`: `STANDARD` for a pool whose
    /// member addresses the account controls, `MANAGED` for one SES manages.
    pub scaling_mode: String,
    /// Tags, in the order they were supplied.
    pub tags: Vec<(String, String)>,
    /// When the pool was created.
    pub created_at: SystemTime,
}

impl DedicatedIpPoolRecord {
    /// A pool as `CreateDedicatedIpPool` stores it.
    ///
    /// A request that names no `ScalingMode` gets [`SCALING_MODE_STANDARD`]: the
    /// member is optional on the create and `@required` on the pool SES reports, and
    /// `PutDedicatedIpPoolScalingAttributes` converts a pool "to a different scaling
    /// mode" with the note that "`MANAGED` pools cannot be converted to `STANDARD`
    /// scaling mode" — so `STANDARD` is the mode a pool starts in and `MANAGED` the
    /// one it can be moved to.
    pub fn new(pool_name: &str, scaling_mode: Option<String>, now: SystemTime) -> Self {
        Self {
            pool_name: pool_name.to_string(),
            scaling_mode: scaling_mode.unwrap_or_else(|| SCALING_MODE_STANDARD.to_string()),
            tags: Vec::new(),
            created_at: now,
        }
    }

    /// The modelled `DedicatedIpPool` view of this record, as `GetDedicatedIpPool`
    /// reports it. One projection, so a write and the read after it cannot disagree.
    pub fn view(&self) -> crate::ops::types::DedicatedIpPool {
        crate::ops::types::DedicatedIpPool {
            pool_name: self.pool_name.clone(),
            scaling_mode: self.scaling_mode.clone(),
        }
    }

    /// The modelled `TagList` view of this record's tags.
    pub fn tag_list(&self) -> Vec<crate::ops::types::Tag> {
        self.tags
            .iter()
            .map(|(key, value)| crate::ops::types::Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// How this service words `AlreadyExistsException` for a dedicated IP pool the account
/// already holds — "the resource specified in your request already exists".
pub fn dedicated_ip_pool_already_exists(pool_name: &str) -> String {
    format!("Dedicated IP pool <{pool_name}> already exists.")
}

/// How this service words `NotFoundException` for a dedicated IP pool it does not hold.
///
/// One wording for every operation that takes a `PoolName`, so a client sees the same
/// sentence whichever call first tells it the pool is not there — the form
/// [`super::configuration_set_not_found`] states for the resource it names: the pool SES
/// interpolated, delimited with angle brackets, in a sentence that is otherwise fixed.
///
/// The fixed text, including the angle brackets around the interpolated pool name, is
/// the sentence a recorded response of the real service answers a missing dedicated IP
/// pool with (the behavioural trace `sesv2.dedicated-ip-pool-lifecycle`, whose report is
/// `issues/things-that-now-work/20261027T223115Z-sesv2-deletededicatedippool-stub.md`);
/// the only variable part, the pool name, comes from the request.
pub fn dedicated_ip_pool_not_found(pool_name: &str) -> String {
    format!("The requested pool <{pool_name}> does not exist.")
}

/// Whether `scaling_mode` is a mode `PutDedicatedIpPoolScalingAttributes` can apply to a
/// dedicated IP pool.
///
/// The operation "convert[s] a dedicated IP pool to a different scaling mode", and
/// `PutDedicatedIpPoolScalingAttributesRequest$ScalingMode` notes that "changing the
/// scaling mode from `MANAGED` to `STANDARD` is not supported" — and a pool that is
/// already `STANDARD` is in the mode a pool starts in, so there is no pool this call
/// could convert TO [`SCALING_MODE_STANDARD`]. The one mode it applies is therefore
/// [`super::SCALING_MODE_MANAGED`], which is also why repeating the conversion answers
/// the same way and changes nothing further.
pub fn scaling_mode_can_be_applied(scaling_mode: &str) -> bool {
    scaling_mode == super::SCALING_MODE_MANAGED
}

/// How this service words `BadRequestException` for a `ScalingMode` the request may not
/// apply ([`scaling_mode_can_be_applied`]).
///
/// Wholly FIXED text that names neither the mode sent nor the pool: the parameter itself
/// is what is being refused. It is the sentence a recorded response of the real service
/// answers such a request with (the behavioural trace
/// `sesv2.dedicated-ip-pool-lifecycle`, whose report is
/// `issues/things-that-now-work/20261028T103411Z-sesv2-putdedicatedippoolscalingattributes-stub.md`),
/// answered there for a pool the account no longer held — so the mode is judged on the
/// request alone, before the pool it names is resolved.
pub fn invalid_scaling_mode() -> String {
    "The ScalingMode parameter is invalid.".to_string()
}

/// The `WarmupPercentage` a dedicated IP in a MANAGED pool reports.
///
/// From `DedicatedIp$WarmupPercentage`: "-1 — Appears for IP addresses in managed
/// dedicated pools where Amazon SES automatically handles the warm-up process,
/// making the percentage not applicable."
pub const MANAGED_WARMUP_PERCENTAGE: i32 = -1;

/// The `WarmupPercentage` of a dedicated IP whose warm-up has finished.
///
/// From `DedicatedIp$WarmupPercentage`: "A value of 100 means the IP address is
/// fully warmed up and ready for use."
pub const WARMUP_COMPLETE_PERCENTAGE: i32 = 100;

/// The `WarmupPercentage` of a dedicated IP whose warm-up has not got anywhere.
///
/// The low end of the `0-100` range `DedicatedIp$WarmupPercentage` documents for a
/// standard dedicated IP, where the member "shows the warm-up completion percentage".
pub const WARMUP_START_PERCENTAGE: i32 = 0;

/// Whether `percentage` is a warm-up percentage a request may associate with a
/// dedicated IP address.
///
/// `Percentage100Wrapper` is a plain integer in the model, so the bound is the one
/// `DedicatedIp$WarmupPercentage` documents for a standard dedicated IP:
/// [`WARMUP_START_PERCENTAGE`]..=[`WARMUP_COMPLETE_PERCENTAGE`]. The one value outside
/// it the member documents, [`MANAGED_WARMUP_PERCENTAGE`], is what SES reports for an
/// address whose warm-up it handles itself, never a progress a caller sets.
pub fn warmup_percentage_can_be_set(percentage: i32) -> bool {
    (WARMUP_START_PERCENTAGE..=WARMUP_COMPLETE_PERCENTAGE).contains(&percentage)
}

/// How this service words `BadRequestException` for a `WarmupPercentage` outside the
/// range [`warmup_percentage_can_be_set`] accepts.
///
/// The sentence names the bounds themselves rather than the number the request carried,
/// so it reads the same for every value it refuses.
pub fn invalid_warmup_percentage() -> String {
    format!(
        "Warmup Percentage must be between {WARMUP_START_PERCENTAGE} and {WARMUP_COMPLETE_PERCENTAGE}."
    )
}

/// One dedicated IP address the account holds: the pool it is in and how far its
/// warm-up has got.
///
/// A dedicated IP is allocated to an account out of band — SES hands one over on a
/// support request, or provisions it into a managed pool — so no request here mints
/// one; the dedicated-IP operations read this table, move an address between pools
/// and move its warm-up along.
#[derive(Debug, Clone)]
pub struct DedicatedIpRecord {
    /// `DedicatedIp$Ip`: the address itself, which is also this record's key.
    pub ip: String,
    /// `DedicatedIp$PoolName`: the dedicated IP pool the address is associated
    /// with, or `None` while it is in none.
    pub pool_name: Option<String>,
    /// `DedicatedIp$WarmupPercentage`: `0`..=[`WARMUP_COMPLETE_PERCENTAGE`] while
    /// SES warms a standard address up, and [`MANAGED_WARMUP_PERCENTAGE`] for an
    /// address in a managed pool, where the percentage does not apply.
    pub warmup_percentage: i32,
}

impl DedicatedIpRecord {
    /// A dedicated IP as the account holds it: an address, the pool it is in, and
    /// the warm-up progress its record carries.
    pub fn new(ip: &str, pool_name: Option<String>, warmup_percentage: i32) -> Self {
        Self {
            ip: ip.to_string(),
            pool_name,
            warmup_percentage,
        }
    }

    /// The modelled `WarmupStatus` of this address, DERIVED from the one stored
    /// number rather than tracked beside it, so the status and the percentage can
    /// never disagree.
    ///
    /// The derivation is the one `DedicatedIp$WarmupStatus` and
    /// `DedicatedIp$WarmupPercentage` document together: a percentage that does not
    /// apply (the managed pools, where "Amazon SES automatically handles the
    /// warm-up process") reports `NOT_APPLICABLE`, a fully warmed up address
    /// ("a value of 100 means the IP address is fully warmed up and ready for use")
    /// reports `DONE`, and anything below that is still warming up, so it reports
    /// `IN_PROGRESS`.
    pub fn warmup_status(&self) -> &'static str {
        if self.warmup_percentage < 0 {
            "NOT_APPLICABLE"
        } else if self.warmup_percentage >= WARMUP_COMPLETE_PERCENTAGE {
            "DONE"
        } else {
            "IN_PROGRESS"
        }
    }

    /// This record as the modelled `DedicatedIp` every operation that reports a
    /// dedicated IP answers with, so they cannot describe one address differently.
    pub fn detail(&self) -> crate::ops::types::DedicatedIp {
        crate::ops::types::DedicatedIp {
            ip: self.ip.clone(),
            warmup_status: self.warmup_status().to_string(),
            warmup_percentage: self.warmup_percentage,
            pool_name: self.pool_name.clone(),
        }
    }
}

/// Whether `ip` is the kind of value an `Ip` names: the shape is documented as "an
/// IPv4 address", so a dotted-quad address is well formed and anything else — a
/// host name, a truncated quad, an octet out of range, an IPv6 address — is not.
pub fn ip_is_well_formed(ip: &str) -> bool {
    ip.parse::<::std::net::Ipv4Addr>().is_ok()
}

/// How this service words `BadRequestException` for an `Ip` that is not an IPv4
/// address at all, stated once for every operation that takes one.
pub fn invalid_ip(ip: &str) -> String {
    format!("Invalid IP address: {ip}. The value has to be an IPv4 address.")
}

/// How this service words `NotFoundException` for an address the account holds no
/// dedicated IP for.
///
/// One wording for every operation that takes an `Ip`, so a client sees the same
/// sentence whichever call first tells it the address is not one of its dedicated
/// IPs. SES delimits the address it interpolated with angle brackets, and says
/// which account it looked in, because an address is a dedicated IP only relative
/// to the account that holds it.
pub fn dedicated_ip_not_found(ip: &str) -> String {
    format!("Could not find dedicated IP <{ip}> under this account.")
}

/// One listing of one address on one third-party blocklist, as SES observed it.
///
/// A blocklist (an RBL) is run by somebody else: it lists an address, and SES reports
/// what it found when it looked. Nothing in this API creates or clears a listing, so
/// the record is written where the observation arrives and the report only reads it.
#[derive(Debug, Clone)]
pub struct BlacklistListingRecord {
    /// `BlacklistEntry$RblName`: "the name of the blacklist that the IP address
    /// appears on".
    pub rbl_name: String,
    /// `BlacklistEntry$ListingTime`: "the time when the blacklisting event occurred".
    pub listing_time: SystemTime,
    /// `BlacklistEntry$Description`: "additional information about the blacklisting
    /// event, as provided by the blacklist maintainer".
    pub description: String,
}

impl BlacklistListingRecord {
    /// One observed listing of an address on a blocklist.
    pub fn new(rbl_name: &str, listing_time: SystemTime, description: &str) -> Self {
        Self {
            rbl_name: rbl_name.to_string(),
            listing_time,
            description: description.to_string(),
        }
    }

    /// This record as the modelled `BlacklistEntry` a report answers with.
    pub fn entry(&self) -> crate::ops::types::BlacklistEntry {
        crate::ops::types::BlacklistEntry {
            rbl_name: Some(self.rbl_name.clone()),
            listing_time: Some(::aws_smithy_types::DateTime::from(self.listing_time)),
            description: Some(self.description.clone()),
        }
    }
}
