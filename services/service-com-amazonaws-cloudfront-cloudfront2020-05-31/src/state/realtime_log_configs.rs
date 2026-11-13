// SPDX-License-Identifier: CC0-1.0

//! The CloudFront REAL-TIME LOG CONFIGURATION slice of this service's state.
//!
//! A real-time log configuration is its own resource kind, so everything about how one is
//! persisted and projected lives here rather than in the parent module: the record a
//! create stores, the ARN a configuration is addressed by, the bound a create is refused
//! at, the rules a submitted configuration has to satisfy, and the modelled errors the
//! real-time log configuration operations answer with. The parent
//! [`ComAmazonawsCloudfrontCloudfront20200531State`] holds the one map
//! (`realtime_log_configs`) and this module holds everything that reads or writes it, so
//! this surface can grow without touching the parts of the store other resource kinds
//! own.
//!
//! Nothing in here is owed by the CLOCK: the model's `RealtimeLogConfig` is `ARN`, `Name`,
//! `SamplingRate`, `EndPoints` and `Fields` only — a configuration carries no status and
//! no propagation deadline, so one this service accepts is immediately complete and the
//! parent's `settle` has nothing to do for it.

use crate::ops::types::{
    DistributionConfig, EndPoint, InvalidArgument, NoSuchRealtimeLogConfig, RealtimeLogConfig,
    RealtimeLogConfigAlreadyExists, TooManyRealtimeLogConfigs,
};

use super::{ComAmazonawsCloudfrontCloudfront20200531State, StoredDistribution};

/// The most real-time log configurations this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`super::MAX_CACHE_POLICIES`]: a
/// configuration record is what a later read answers from, so it cannot be evicted, and
/// the create path is refused at this entry count with the modelled
/// `TooManyRealtimeLogConfigs` ("You have reached the maximum number of real-time log
/// configurations for this Amazon Web Services account") instead of growing further.
/// CloudFront's own ceiling is an account quota that can be raised and the model states no
/// number, so this bound is the emulator's rather than a claim about the service.
pub const MAX_REALTIME_LOG_CONFIGS: usize = 1_000;

/// The smallest sampling rate a real-time log configuration may carry.
///
/// `CreateRealtimeLogConfigRequest$SamplingRate`: "You can specify a whole number between
/// 1 and 100 (inclusive) to determine the percentage of viewer requests that are
/// represented in the real-time log data." The member carries no `@range` in the model, so
/// the bound is not refused before a handler runs and the create path checks it itself.
pub const MIN_SAMPLING_RATE: i64 = 1;

/// The largest sampling rate a real-time log configuration may carry — the other end of
/// the range [`MIN_SAMPLING_RATE`] opens, a percentage of viewer requests.
pub const MAX_SAMPLING_RATE: i64 = 100;

/// The one `EndPoint$StreamType` a real-time log configuration may name: "The type of data
/// stream where you are sending real-time log data. The only valid value is `Kinesis`."
pub const KINESIS_STREAM_TYPE: &str = "Kinesis";

/// The most endpoints ONE real-time log configuration may name.
///
/// CHOSEN by this emulator: a stored configuration is what every later read answers from,
/// so the record retains every endpoint the request listed and nothing evicts them — which
/// makes the length of that list a per-request growth of the store that has to be bounded
/// where it enters. It sits far above any configuration CloudFront serves (a real-time log
/// configuration sends to a Kinesis data stream, and the model states no number), so a
/// caller never meets it by accident, and over it the refusal is the modelled
/// `InvalidArgument`.
pub const MAX_REALTIME_LOG_CONFIG_END_POINTS: usize = 25;

/// Every field CloudFront can include in a real-time log record, in the ONE order it sends
/// them in.
///
/// The catalog, and the order, are the real-time log configuration fields of the CloudFront
/// Developer Guide ("Real-time log configuration fields", the list
/// `RealtimeLogConfig$Fields` points a reader at), read in the order that list states them.
/// That order is the service's, not the request's: `RealtimeLogConfig$Fields` is "a list of
/// fields that are included in each real-time log record. In an API response, the fields are
/// provided in the same order in which they are sent to the Amazon Kinesis data stream" — so
/// a configuration holds the submitted field SET in this order ([`canonical_field_order`]),
/// every read reports it in this order, and each delivered record carries its values in that
/// same order.
pub const REALTIME_LOG_FIELDS: [&str; 45] = [
    "timestamp",
    "c-ip",
    "time-to-first-byte",
    "sc-status",
    "sc-bytes",
    "cs-method",
    "cs-protocol",
    "cs-host",
    "cs-uri-stem",
    "cs-bytes",
    "x-edge-location",
    "x-edge-request-id",
    "x-host-header",
    "time-taken",
    "cs-protocol-version",
    "c-ip-version",
    "cs-user-agent",
    "cs-referer",
    "cs-cookie",
    "cs-uri-query",
    "x-edge-response-result-type",
    "x-forwarded-for",
    "ssl-protocol",
    "ssl-cipher",
    "x-edge-result-type",
    "fle-encrypted-fields",
    "fle-status",
    "sc-content-type",
    "sc-content-len",
    "sc-range-start",
    "sc-range-end",
    "c-port",
    "x-edge-detailed-result-type",
    "c-country",
    "cs-accept-encoding",
    "cs-accept",
    "cache-behavior-path-pattern",
    "headers",
    "header-names",
    "headers-count",
    "primary-distribution-id",
    "primary-distribution-dns-name",
    "origin-fbl",
    "origin-lbl",
    "asn",
];

/// The submitted field list as a configuration HOLDS it: the submitted SET, in
/// [`REALTIME_LOG_FIELDS`] order.
///
/// A configuration carries "a list of fields to include in each real-time log record", and a
/// field is either included or not, so the list is a SET: a name submitted twice is held
/// once. The ORDER a configuration holds is the service's own — the order the fields are sent
/// to the data stream, which is the order a read reports — so the submitted order decides
/// nothing. A name the catalog does not carry has no position in that order, so it keeps the
/// one it was submitted in, after every name that does.
pub fn canonical_field_order(fields: &[String]) -> Vec<String> {
    let mut ordered: Vec<String> = REALTIME_LOG_FIELDS
        .iter()
        .filter(|known| fields.iter().any(|field| field == *known))
        .map(|known| (*known).to_string())
        .collect();
    for field in fields {
        let known = REALTIME_LOG_FIELDS.contains(&field.as_str());
        if !known && !ordered.iter().any(|held| held == field) {
            ordered.push(field.clone());
        }
    }
    ordered
}

/// The whole of a percentage, which a `SamplingRate` is stated as.
///
/// `CreateRealtimeLogConfigRequest$SamplingRate` is "a whole number between 1 and 100
/// (inclusive) to determine the percentage of viewer requests that are represented in the
/// real-time log data", so a rate is that many hundredths of the requests a configuration
/// covers ([`StoredRealtimeLogConfig::represents_next_request`]).
pub const WHOLE_SAMPLING_PERCENT: i64 = MAX_SAMPLING_RATE;

/// The most fields ONE real-time log configuration may include in a log record.
///
/// CHOSEN by this emulator, for the same reason and in the same way as
/// [`MAX_REALTIME_LOG_CONFIG_END_POINTS`]: the stored record retains the whole list, so its
/// length is bounded where it enters. It sits well above the set of field names CloudFront
/// documents for a real-time log record, so a caller never meets it by accident.
pub const MAX_REALTIME_LOG_CONFIG_FIELDS: usize = 100;

/// One CloudFront real-time log configuration, as this emulator persists it.
///
/// The record is the single source of truth for the configuration's name, the ARN minted
/// for it and the three things it was created with, so the create and every later read
/// answer from it rather than from the request being served.
#[derive(Debug, Clone)]
pub struct StoredRealtimeLogConfig {
    /// `RealtimeLogConfig$Name` — "The unique name of this real-time log configuration",
    /// which is also the key this record is held under.
    pub name: String,
    /// `RealtimeLogConfig$ARN` — "The Amazon Resource Name (ARN) of this real-time log
    /// configuration", minted once by the create ([`realtime_log_config_arn`]) and stored,
    /// so every later read reports the same ARN and an ARN lookup compares against the
    /// stored value.
    pub arn: String,
    /// `RealtimeLogConfig$SamplingRate` — the percentage of viewer requests represented in
    /// the log data, as submitted.
    pub sampling_rate: i64,
    /// `RealtimeLogConfig$EndPoints` — the data streams the log records are sent to,
    /// stored whole so a read reports what was stored rather than re-deriving anything
    /// from a request.
    pub end_points: Vec<EndPoint>,
    /// `RealtimeLogConfig$Fields` — "In an API response, the fields are provided in the
    /// same order in which they are sent to the Amazon Kinesis data stream", so the stored
    /// ORDER is the one the records are sent in ([`canonical_field_order`]) and a read
    /// reports that.
    pub fields: Vec<String>,
    /// How much of a record's worth of sampling the requests this configuration has already
    /// covered add up to, in hundredths of a request — bookkeeping of this emulator's, which
    /// no response carries ([`StoredRealtimeLogConfig::represents_next_request`]).
    pub sampling_cursor: i64,
}

impl StoredRealtimeLogConfig {
    /// The `RealtimeLogConfig` projection every real-time log configuration operation
    /// returns, read from the one stored record.
    pub fn to_realtime_log_config(&self) -> RealtimeLogConfig {
        RealtimeLogConfig {
            arn: self.arn.clone(),
            name: self.name.clone(),
            sampling_rate: self.sampling_rate,
            end_points: self.end_points.clone(),
            fields: self.fields.clone(),
        }
    }

    /// Whether the NEXT viewer request this configuration covers is one of the requests its
    /// log data represents, advancing the configuration's own sampling cursor.
    ///
    /// `SamplingRate` is "a whole number between 1 and 100 (inclusive) to determine the
    /// percentage of viewer requests that are represented in the real-time log data", so of
    /// every hundred requests a configuration covers, exactly that many are represented: each
    /// covered request adds the rate to the cursor, and a record is delivered — and a whole
    /// percent spent — as soon as the cursor has a whole request's worth in it. What is left
    /// over CARRIES to the next request, which is what makes the long-run share the rate
    /// itself at every value of it: 100 represents every request, 75 three in four, 50 every
    /// second one, and 1 one in a hundred. No randomness, and the cursor stays below one
    /// whole percent plus the rate, which the create and update paths hold to
    /// [`MAX_SAMPLING_RATE`].
    pub fn represents_next_request(&mut self) -> bool {
        self.sampling_cursor = self.sampling_cursor.saturating_add(self.sampling_rate);
        if self.sampling_cursor < WHOLE_SAMPLING_PERCENT {
            return false;
        }
        self.sampling_cursor -= WHOLE_SAMPLING_PERCENT;
        true
    }
}

impl ComAmazonawsCloudfrontCloudfront20200531State {
    /// The real-time log configuration `name` names, or `None` when the store holds none.
    pub fn realtime_log_config(&self, name: &str) -> Option<&StoredRealtimeLogConfig> {
        self.realtime_log_configs.get(name)
    }

    /// The real-time log configuration `arn` names, or `None` when the store holds none.
    ///
    /// The ARN a configuration carries is the one minted when it was created, so the
    /// lookup compares against the STORED value rather than re-deriving a name out of the
    /// request's ARN. A scan over the stored records rather than a second index beside
    /// them: the records are the single source of truth for which ARNs exist, so a scan
    /// cannot drift out of step with them, and the collection is bounded by
    /// [`MAX_REALTIME_LOG_CONFIGS`].
    pub fn realtime_log_config_by_arn(&self, arn: &str) -> Option<&StoredRealtimeLogConfig> {
        self.realtime_log_configs
            .values()
            .find(|config| config.arn == arn)
    }

    /// The configuration a request addresses by `name`, by `arn`, or by both.
    ///
    /// "To get a real-time log configuration, you can provide the configuration's name or
    /// its Amazon Resource Name (ARN). You must provide at least one. If you provide both,
    /// CloudFront uses the name to identify the real-time log configuration to get" — so
    /// the NAME is what identifies the configuration whenever the request carries one, and
    /// the ARN is consulted only for a request that carries none. An identifier submitted
    /// as an empty element names nothing, so it counts as not supplied rather than as a
    /// name no configuration holds.
    ///
    /// ONE rule for every operation that addresses a configuration either way, so the
    /// reads and the mutations cannot drift apart.
    pub fn addressed_realtime_log_config(
        &self,
        name: Option<&str>,
        arn: Option<&str>,
    ) -> Result<&StoredRealtimeLogConfig, RealtimeLogConfigAddress> {
        let name = name.filter(|name| !name.is_empty());
        let arn = arn.filter(|arn| !arn.is_empty());
        let found = match (name, arn) {
            (Some(name), _) => self.realtime_log_config(name),
            (None, Some(arn)) => self.realtime_log_config_by_arn(arn),
            (None, None) => return Err(RealtimeLogConfigAddress::NoIdentifier),
        };
        found.ok_or(RealtimeLogConfigAddress::NotFound)
    }

    /// The real-time log configuration `arn` names, to be MUTATED: the record is the one
    /// [`Self::realtime_log_config_by_arn`] answers, reached the same way, for the edge's own
    /// sampling cursor ([`StoredRealtimeLogConfig::represents_next_request`]), which is the
    /// only part of a configuration anything but an update writes.
    pub fn realtime_log_config_by_arn_mut(
        &mut self,
        arn: &str,
    ) -> Option<&mut StoredRealtimeLogConfig> {
        self.realtime_log_configs
            .values_mut()
            .find(|config| config.arn == arn)
    }

    /// The distribution a cache behaviour of which still sends its logs through the
    /// real-time log configuration `arn` names, or `None` when none does.
    ///
    /// "You cannot delete a real-time log configuration if it's attached to a cache
    /// behavior", which is the rule [`references_realtime_log_config`] states for one
    /// distribution's whole configuration. A scan over the stored distributions: the
    /// stored configurations are the single source of truth for which attachments exist, so
    /// a scan cannot drift out of step with them, and the collection is bounded by
    /// [`super::MAX_DISTRIBUTIONS`].
    pub fn distribution_using_realtime_log_config(&self, arn: &str) -> Option<&StoredDistribution> {
        self.distributions
            .values()
            .find(|distribution| references_realtime_log_config(&distribution.config, arn))
    }
}

/// Why a request addresses no stored real-time log configuration.
///
/// The two conditions are different refusals — one is about the request, the other about
/// the store — so the caller reports each with the error its own model declares for it
/// rather than collapsing both into one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeLogConfigAddress {
    /// The request named neither a `Name` nor an `ARN`, so it addresses no configuration
    /// at all: "You must provide at least one."
    NoIdentifier,
    /// The identifier the request named addresses no configuration this store holds.
    NotFound,
}

/// The modelled `InvalidArgument` (HTTP 400) for a request that names neither identifier,
/// so it addresses no real-time log configuration at all: "To get a real-time log
/// configuration, you can provide the configuration's name or its Amazon Resource Name
/// (ARN). You must provide at least one."
///
/// ONE CONSTANT for the condition rather than a sentence built from the request: what the
/// refusal states is the CONSTRAINT on the parameters, which is a property of the
/// operation and not of the call.
pub fn realtime_log_config_identifier_required() -> InvalidArgument {
    InvalidArgument {
        message: Some(REALTIME_LOG_CONFIG_IDENTIFIER_REQUIRED_MESSAGE.to_string()),
    }
}

/// The message a request naming neither identifier is refused with, on the wire.
///
/// The value is the WIRE text — what the recorded service answers this code with, and what
/// this emulator therefore serves, as [`super::DISTRIBUTION_NOT_DISABLED_MESSAGE`] is for
/// its own condition. It names the two parameters rather than the request, so every
/// refusal of this condition reads the same.
pub const REALTIME_LOG_CONFIG_IDENTIFIER_REQUIRED_MESSAGE: &str =
    "The parameter Name/ARN must be provided.";

/// The modelled `InvalidArgument` (HTTP 400) for a DISTRIBUTION LISTING that names neither
/// identifier, so it addresses no real-time log configuration at all: "You can specify the
/// real-time log configuration by its name or its Amazon Resource Name (ARN). You must
/// provide at least one."
///
/// The CONDITION is the one [`realtime_log_config_identifier_required`] states, and the
/// constraint stated is the same; what differs is the SENTENCE the service answers it with
/// on this surface ([`REALTIME_LOG_CONFIG_LISTING_IDENTIFIER_REQUIRED_MESSAGE`]), so each
/// operation's refusal reads as the service's own does rather than one being reworded to
/// match the other.
pub fn realtime_log_config_listing_identifier_required() -> InvalidArgument {
    InvalidArgument {
        message: Some(REALTIME_LOG_CONFIG_LISTING_IDENTIFIER_REQUIRED_MESSAGE.to_string()),
    }
}

/// The message a distribution listing naming neither identifier is refused with, on the
/// wire.
///
/// The value is the WIRE text — what the recorded service answers this code with on the
/// distribution-listing surface, which is the shorter of the two forms the two surfaces
/// word this one constraint in. It names the two parameters rather than the request, so
/// every refusal of this condition reads the same.
pub const REALTIME_LOG_CONFIG_LISTING_IDENTIFIER_REQUIRED_MESSAGE: &str =
    "Name or ARN must be provided.";

/// The ARN of a real-time log configuration. CloudFront is a GLOBAL service, so the ARN's
/// region segment is empty: `arn:aws:cloudfront::<account>:realtime-log-config/<Name>`.
/// The resource part is the configuration's name, which is the identifier a cache
/// behaviour's `RealtimeLogConfigArn` names one by.
pub fn realtime_log_config_arn(name: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("realtime-log-config/{name}"))
}

/// Why `sampling_rate`, `end_points` and `fields` describe no real-time log configuration
/// this service accepts, or `None` when they do.
///
/// ONE rule for every operation that submits a real-time log configuration, so a create
/// and a later update cannot drift apart. Two documented constraints the model does not
/// carry as traits, and so are not refused before a handler runs:
///
/// * the sampling rate is "a whole number between 1 and 100 (inclusive)", a percentage of
///   viewer requests ([`MIN_SAMPLING_RATE`]..=[`MAX_SAMPLING_RATE`]);
/// * an endpoint's stream type is the one value the member admits
///   ([`KINESIS_STREAM_TYPE`]), and an endpoint of that type carries the stream
///   configuration that names where the records go — "Contains information about the
///   Amazon Kinesis data stream where you are sending real-time log data" — so an endpoint
///   without one names no destination at all.
///
/// A configuration also describes where the log data goes and what each record holds, so a
/// list that is EMPTY describes no configuration either: a configuration "send[s] real-time
/// log data to the specified Amazon Kinesis data stream", and its fields are the ones
/// "include[d] in each real-time log record".
///
/// Then the two lengths this emulator bounds the stored record by
/// ([`MAX_REALTIME_LOG_CONFIG_END_POINTS`], [`MAX_REALTIME_LOG_CONFIG_FIELDS`]), so no
/// single request chooses how large a record the store retains — checked BEFORE the walk
/// over the endpoints, so the per-endpoint work is bounded too.
///
/// The refusal is the `InvalidArgument` every operation that submits a configuration models,
/// built whole here. The rules CloudFront PUBLISHES for a parameter carry the service's own
/// sentence for that parameter — the sampling rate's range
/// ([`sampling_rate_out_of_range_message`]) and the two lists that have to name something
/// ([`empty_list_parameter_message`]) — rather than a sentence of this emulator's, while the
/// bounds that are this emulator's OWN are worded as such ([`invalid_realtime_log_config`]).
pub fn realtime_log_config_refusal(
    sampling_rate: i64,
    end_points: &[EndPoint],
    fields: &[String],
) -> Option<InvalidArgument> {
    if !(MIN_SAMPLING_RATE..=MAX_SAMPLING_RATE).contains(&sampling_rate) {
        return Some(InvalidArgument {
            message: Some(sampling_rate_out_of_range_message()),
        });
    }
    if end_points.is_empty() {
        return Some(InvalidArgument {
            message: Some(empty_list_parameter_message("EndPoints", "endpoint")),
        });
    }
    if fields.is_empty() {
        return Some(InvalidArgument {
            message: Some(empty_list_parameter_message("Fields", "field")),
        });
    }
    if end_points.len() > MAX_REALTIME_LOG_CONFIG_END_POINTS {
        return Some(invalid_realtime_log_config(&format!(
            "a real-time log configuration names at most {MAX_REALTIME_LOG_CONFIG_END_POINTS} \
             endpoints but the request names {}",
            end_points.len()
        )));
    }
    if fields.len() > MAX_REALTIME_LOG_CONFIG_FIELDS {
        return Some(invalid_realtime_log_config(&format!(
            "a real-time log configuration includes at most {MAX_REALTIME_LOG_CONFIG_FIELDS} \
             fields but the request includes {}",
            fields.len()
        )));
    }
    // Only now walked, so the per-endpoint work is bounded by the length checked above.
    for end_point in end_points {
        if end_point.stream_type != KINESIS_STREAM_TYPE {
            return Some(invalid_realtime_log_config(&format!(
                "the only valid EndPoint StreamType is {KINESIS_STREAM_TYPE} but the request \
                 submits {}",
                end_point.stream_type
            )));
        }
        if end_point.kinesis_stream_config.is_none() {
            return Some(invalid_realtime_log_config(&format!(
                "an EndPoint of StreamType {KINESIS_STREAM_TYPE} names the Kinesis data stream \
                 to send to, but the request submits one with no KinesisStreamConfig"
            )));
        }
    }
    None
}

/// The sentence CloudFront refuses a list parameter submitted with no entries with, on the
/// wire: it names the MEMBER the list arrived in and what one entry of it is, and states what
/// the parameter has to carry rather than anything the request submitted — so every refusal of
/// this condition reads the same.
///
/// One producer for every list of this service a request has to name something in, so no two
/// of them can word the same refusal differently.
pub fn empty_list_parameter_message(parameter: &str, item: &str) -> String {
    format!("The parameter {parameter} must contain at least one {item}.")
}

/// The sentence CloudFront refuses a `SamplingRate` outside the range it publishes for the
/// parameter with, on the wire.
///
/// The text states the CONSTRAINT the parameter carries — the service names the parameter and
/// the two ends of its range — and nothing the request submitted reaches it, so every refusal
/// of this condition reads the same whatever value was sent. The ends come from the range
/// this service admits ([`MIN_SAMPLING_RATE`]..=[`MAX_SAMPLING_RATE`]): CloudFront words the
/// lower end EXCLUSIVELY ("greater than") and the upper end inclusively, so the lower end is
/// written as the value just below the smallest rate a configuration may carry.
pub fn sampling_rate_out_of_range_message() -> String {
    format!(
        "The parameter SamplingRate must be an integer greater than {} and less than or equal \
         to {MAX_SAMPLING_RATE}.",
        MIN_SAMPLING_RATE - 1
    )
}

/// The modelled `InvalidArgument` (HTTP 400) an operation whose model serves no
/// `NoSuchRealtimeLogConfig` answers a `Name` or `ARN` the store holds no configuration for.
///
/// The CONDITION is the one [`no_such_realtime_log_config`] reports — no configuration
/// answers to the identifier — so the SENTENCE is that condition's
/// ([`NO_SUCH_REALTIME_LOG_CONFIG_MESSAGE`]) whichever code the operation's model makes it
/// carry, and nothing from the request belongs in it.
pub fn invalid_argument_no_such_realtime_log_config() -> InvalidArgument {
    InvalidArgument {
        message: Some(NO_SUCH_REALTIME_LOG_CONFIG_MESSAGE.to_string()),
    }
}

/// The reason a create whose `Name` is empty is refused with.
///
/// A configuration is IDENTIFIED by its name — "A unique name to identify this real-time
/// log configuration" — and
/// [`ComAmazonawsCloudfrontCloudfront20200531State::addressed_realtime_log_config`] treats
/// an empty identifier as none at all, so an empty name identifies nothing and names no
/// configuration a later call could address.
pub const REALTIME_LOG_CONFIG_NAME_REQUIRED: &str =
    "a real-time log configuration is identified by its Name, so the Name must not be empty";

/// The modelled `InvalidArgument` (HTTP 400) for a real-time log configuration that breaks
/// one of the rules [`realtime_log_config_refusal`] states, worded as the model documents
/// the shape ("An argument is invalid.") followed by the constraint the request broke.
pub fn invalid_realtime_log_config(reason: &str) -> InvalidArgument {
    InvalidArgument {
        message: Some(format!("An argument is invalid: {reason}")),
    }
}

/// The modelled `InvalidArgument` (HTTP 400) for a listing `Marker` that is no resume point
/// this service ever handed out.
///
/// `ListRealtimeLogConfigsRequest$Marker` is "the value of `NextMarker` from the current
/// page's response", which names a configuration on that page; a configuration is
/// identified by a name that is never empty ([`REALTIME_LOG_CONFIG_NAME_REQUIRED`]), so an
/// empty value is no resume point rather than a name the store holds nothing under. What the
/// refusal states is the CONSTRAINT on the parameter, so nothing from the request belongs in
/// the text and every refusal of this condition reads the same.
pub fn invalid_realtime_log_config_marker() -> InvalidArgument {
    InvalidArgument {
        message: Some(REALTIME_LOG_CONFIG_MARKER_INVALID_MESSAGE.to_string()),
    }
}

/// The message a listing whose `Marker` is no resume point is refused with.
pub const REALTIME_LOG_CONFIG_MARKER_INVALID_MESSAGE: &str =
    "Marker must be the name of a real-time log configuration, as returned in NextMarker by \
     ListRealtimeLogConfigs.";

/// The modelled `RealtimeLogConfigAlreadyExists` (HTTP 409) for a configuration name that
/// is already taken.
///
/// The condition is a property of the STORE — a configuration already holds the name — so
/// nothing from the request belongs in the text and every refusal of it reads the same.
pub fn realtime_log_config_already_exists() -> RealtimeLogConfigAlreadyExists {
    RealtimeLogConfigAlreadyExists {
        message: Some(REALTIME_LOG_CONFIG_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The message a create under a name that is already taken is refused with, on the wire.
///
/// The value is the WIRE text, which is the first sentence of what the model documents on
/// the shape ("A real-time log configuration with this name already exists. You must
/// provide a unique name. To modify an existing real-time log configuration, use
/// `UpdateRealtimeLogConfig`.") — the served sentence states the condition and the
/// documentation goes on to advise. Do not extend this constant to the whole
/// documentation paragraph.
pub const REALTIME_LOG_CONFIG_ALREADY_EXISTS_MESSAGE: &str =
    "A real-time log configuration with this name already exists.";

/// The modelled `TooManyRealtimeLogConfigs` (HTTP 400) for a create that would take the
/// store past [`MAX_REALTIME_LOG_CONFIGS`].
///
/// The sentence states the bound this emulator holds rather than quoting a service quota
/// it does not know, for the same reason the bound itself is the emulator's.
pub fn too_many_realtime_log_configs() -> TooManyRealtimeLogConfigs {
    TooManyRealtimeLogConfigs {
        message: Some(format!(
            "This emulator holds at most {MAX_REALTIME_LOG_CONFIGS} real-time log \
             configurations; delete one before creating another"
        )),
    }
}

/// The modelled `NoSuchRealtimeLogConfig` (HTTP 404) for a name or ARN the store holds no
/// configuration for.
///
/// The sentence is a property of the STORE rather than of the request — no configuration
/// answers to the identifier — so nothing from the request belongs in the text and every
/// refusal of this condition reads the same.
pub fn no_such_realtime_log_config() -> NoSuchRealtimeLogConfig {
    NoSuchRealtimeLogConfig {
        message: Some(NO_SUCH_REALTIME_LOG_CONFIG_MESSAGE.to_string()),
    }
}

/// `NoSuchRealtimeLogConfig`'s message, in the form CloudFront answers a `NoSuch…` with for
/// a named resource — "The specified <resource> does not exist.", as
/// [`super::NO_SUCH_CACHE_POLICY_MESSAGE`] carries it. The recorded service answers this
/// code with that sentence, and the shape's own documentation states it in its shorter form
/// ("The real-time log configuration does not exist."); this is the served one.
pub const NO_SUCH_REALTIME_LOG_CONFIG_MESSAGE: &str =
    "The specified real-time log configuration does not exist.";

/// The modelled `RealtimeLogConfigInUse` (HTTP 400) for a delete of a configuration a
/// cache behaviour still sends records through.
///
/// FIXED service text ([`REALTIME_LOG_CONFIG_IN_USE_MESSAGE`]) — a property of the
/// CONDITION rather than of the request, so nothing from the request or the store belongs
/// in it and every refusal of this condition reads the same.
pub fn realtime_log_config_in_use() -> crate::ops::types::RealtimeLogConfigInUse {
    crate::ops::types::RealtimeLogConfigInUse {
        message: Some(REALTIME_LOG_CONFIG_IN_USE_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront answers `RealtimeLogConfigInUse` with, on the wire.
///
/// The WIRE text — what the recorded service answers this code with, and what this
/// emulator therefore serves. Like [`super::DISTRIBUTION_NOT_DISABLED_MESSAGE`] it differs
/// from the sentence the MODEL's documentation of the error shape uses for the same
/// condition ("Cannot delete the real-time log configuration because it is attached to one
/// or more cache behaviors."): that one documents the condition, this one is served. Do
/// not "correct" this constant to the documentation sentence.
pub const REALTIME_LOG_CONFIG_IN_USE_MESSAGE: &str =
    "The CloudFront real-time log configuration is still being used.";

/// Whether `config` sends any of its cache behaviours' logs through the real-time log
/// configuration `arn` names.
///
/// A real-time log configuration is attached to a distribution by a cache behaviour's
/// `RealtimeLogConfigArn`, and a distribution's behaviours are its DEFAULT cache behaviour
/// together with its ordered ones — both declare the member, so both are walked here and a
/// distribution is attached to the configuration as soon as ONE of them names it. The same
/// rule the parent module's `references_cache_policy` states for the other resource a cache
/// behaviour attaches.
pub fn references_realtime_log_config(config: &DistributionConfig, arn: &str) -> bool {
    super::behavior_attachments(
        config,
        |behavior| behavior.realtime_log_config_arn.as_deref(),
        |behavior| behavior.realtime_log_config_arn.as_deref(),
    )
    .any(|attached| attached == arn)
}
