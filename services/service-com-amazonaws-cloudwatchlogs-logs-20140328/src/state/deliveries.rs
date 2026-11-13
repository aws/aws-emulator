// SPDX-License-Identifier: CC0-1.0

use super::*;

/// What identifies one vended-log delivery resource in this store: the Region it lives in,
/// and the name — or the id — it is known by within that Region.
///
/// A delivery source, a delivery destination and a delivery are REGIONAL resources: each is
/// created through one Region's endpoint, its ARN carries that Region
/// ([`delivery_source_arn`], [`delivery_destination_arn`], [`delivery_arn`]), and the
/// uniqueness each operation documents for a name ("This name must be unique for all delivery
/// sources in your account") holds within that Region — so the same name in two Regions is two
/// separate resources, and an endpoint resolves only the resources of its own Region. The
/// Region comes first in the ordering, so one Region's resources enumerate in name order and a
/// reporting operation's page token stays a position in that order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegionalKey {
    /// The Region the resource was created in — the one the creating request's credential
    /// scope named ([`request_region`]).
    pub region: String,
    /// What identifies the resource within that Region: a delivery source's or destination's
    /// name, or a delivery's id.
    pub name: String,
}

impl RegionalKey {
    /// The key of the resource `name` names in `region`.
    pub fn new(region: &str, name: &str) -> Self {
        Self {
            region: region.to_string(),
            name: name.to_string(),
        }
    }
}

/// One delivery — "a connection between a logical delivery source and a logical delivery
/// destination" — as it is stored. Every member of the model's `Delivery` shape is
/// reported from this record, so `DescribeDeliveries` answers from state alone.
#[derive(Debug, Clone)]
pub struct DeliveryRecord {
    /// `Delivery$id` — "The unique ID that identifies this delivery in your account."
    /// Also the name half of the key the store holds this delivery under ([`RegionalKey`]).
    pub id: String,
    /// `Delivery$arn` — "The Amazon Resource Name (ARN) that uniquely identifies this
    /// delivery", as [`delivery_arn`] builds it.
    pub arn: String,
    /// `Delivery$deliverySourceName` — "The name of the delivery source that is associated
    /// with this delivery."
    pub delivery_source_name: String,
    /// `Delivery$deliveryDestinationArn` — "The ARN of the delivery destination that is
    /// associated with this delivery."
    pub delivery_destination_arn: String,
    /// `Delivery$deliveryDestinationType` — the vendor the destination resource belongs to
    /// ([`delivery_destination_type_for_arn`]); `None` for a destination ARN naming none of
    /// the four the model's `DeliveryDestinationType` enumerates.
    pub delivery_destination_type: Option<String>,
    /// `Delivery$recordFields` — "The record fields used in this delivery". `None` when the
    /// create named no list at all, which is not the same as the empty list it may name
    /// (`RecordFields` is `@length(min: 0)`).
    pub record_fields: Option<Vec<String>>,
    /// `Delivery$fieldDelimiter` — "The field delimiter that is used between record fields
    /// when the final output format of a delivery is in Plain, W3C, or Raw format."
    pub field_delimiter: Option<String>,
    /// `Delivery$s3DeliveryConfiguration` — the configuration that applies "only when the
    /// delivery destination resource is an S3 bucket".
    pub s3_delivery_configuration: Option<crate::ops::types::S3DeliveryConfiguration>,
    /// `Delivery$tags` — "The tags that have been assigned to this delivery." A `BTreeMap`
    /// so the stored order is stable; `Tags` is `@length(min: 1)`, so a delivery carrying
    /// none reports no member at all.
    pub tags: BTreeMap<String, String>,
}

impl DeliveryRecord {
    /// A brand-new delivery between one delivery source and one delivery destination, with
    /// the ARN and the destination type DERIVED rather than taken from the caller: both are
    /// facts about the resources being connected. This is what `CreateDelivery` stores once
    /// it is implemented (it is still a scaffold stub), which is also the only caller this
    /// constructor is waiting for.
    ///
    /// The optional members a create may name — `recordFields`, `fieldDelimiter`,
    /// `s3DeliveryConfiguration`, `tags` — are left at their "the request named none" value
    /// and set on the returned record by the caller that has them.
    pub fn new(
        id: &str,
        region: &str,
        delivery_source_name: &str,
        delivery_destination_arn: &str,
    ) -> Self {
        Self {
            id: id.to_string(),
            arn: delivery_arn(region, id),
            delivery_source_name: delivery_source_name.to_string(),
            delivery_destination_arn: delivery_destination_arn.to_string(),
            delivery_destination_type: delivery_destination_type_for_arn(delivery_destination_arn)
                .map(ToString::to_string),
            record_fields: None,
            field_delimiter: None,
            s3_delivery_configuration: None,
            tags: BTreeMap::new(),
        }
    }

    /// Resolve this delivery's EFFECTIVE configuration, from the configuration template that
    /// governs it ([`delivery_configuration_template`]) and the object layout its SENDER
    /// publishes ([`delivery_object_layout`]).
    ///
    /// A delivery delivers under one configuration whether or not the request that created it
    /// stated the whole of it: a member the request named is the caller's, and a member it did
    /// not name takes the value the template publishes as the default for this log type and
    /// this kind of destination ("the valid and default values that are used when creating
    /// delivery sources, delivery destinations, and deliveries"). So every reader of the
    /// delivery reports what will actually be delivered rather than which members of the
    /// create happened to be filled in, and a caller can tell a defaulted value from a
    /// configured one by comparing it with the template.
    ///
    /// The two members that apply to every delivery are resolved even where the GOVERNING
    /// template publishes no default for them: a delivery that states no delimiter joins its
    /// record fields with none ([`CONFIGURATION_TEMPLATE_DEFAULT_FIELD_DELIMITER`]), and the
    /// object layout comes from the SENDER rather than from the destination this delivery pairs
    /// it with — the layout is published per log type, so a delivery to a destination that is
    /// not an S3 bucket reports the same suffix path as one to a bucket instead of losing it,
    /// with the Hive-compatible prefix structure off where nothing published says otherwise
    /// ([`CONFIGURATION_TEMPLATE_DEFAULT_HIVE_COMPATIBLE_PATH`]). A sender that publishes no
    /// prefix of its own, and one the catalogue publishes nothing for, leave the suffix path at
    /// no prefix at all. `recordFields` is left unreported when no template governs the
    /// delivery: which fields its records carry is published by the sending service, and
    /// nothing published says.
    pub fn resolve_configuration(
        &mut self,
        template: Option<&crate::ops::types::ConfigurationTemplate>,
        object_layout: Option<&crate::ops::types::S3DeliveryConfiguration>,
    ) {
        let defaults = template.and_then(|template| template.default_delivery_config_values.as_ref());
        if self.record_fields.is_none() {
            self.record_fields = defaults.and_then(|defaults| defaults.record_fields.clone());
        }
        if self.field_delimiter.is_none() {
            self.field_delimiter = Some(
                defaults
                    .and_then(|defaults| defaults.field_delimiter.clone())
                    .unwrap_or_else(|| CONFIGURATION_TEMPLATE_DEFAULT_FIELD_DELIMITER.to_string()),
            );
        }
        let configured = self.s3_delivery_configuration.take();
        self.s3_delivery_configuration = Some(crate::ops::types::S3DeliveryConfiguration {
            enable_hive_compatible_path: Some(
                configured
                    .as_ref()
                    .and_then(|configured| configured.enable_hive_compatible_path)
                    .or_else(|| {
                        object_layout.and_then(|published| published.enable_hive_compatible_path)
                    })
                    .unwrap_or(CONFIGURATION_TEMPLATE_DEFAULT_HIVE_COMPATIBLE_PATH),
            ),
            suffix_path: Some(
                configured
                    .as_ref()
                    .and_then(|configured| configured.suffix_path.clone())
                    .or_else(|| object_layout.and_then(|published| published.suffix_path.clone()))
                    .unwrap_or_default(),
            ),
        });
    }

    /// This delivery as the model's `Delivery` structure, which is what `CreateDelivery`,
    /// `GetDelivery` and `DescribeDeliveries` all report, so one record renders the same way
    /// through any of them.
    pub fn reported(&self) -> crate::ops::types::Delivery {
        crate::ops::types::Delivery {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            delivery_source_name: Some(self.delivery_source_name.clone()),
            delivery_destination_arn: Some(self.delivery_destination_arn.clone()),
            delivery_destination_type: self.delivery_destination_type.clone(),
            record_fields: self.record_fields.clone(),
            field_delimiter: self.field_delimiter.clone(),
            s3_delivery_configuration: self.s3_delivery_configuration.clone(),
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
        }
    }
}

/// A delivery's ARN, the form `Delivery$arn` ("The Amazon Resource Name (ARN) that uniquely
/// identifies this delivery") carries: a `delivery` resource of this service, identified by
/// the delivery's own id.
pub fn delivery_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("delivery:{id}"))
}

/// The id `CreateDelivery` gives the delivery it creates — "The unique ID that identifies
/// this delivery in your account" — minted from the store's monotonic delivery counter so it
/// is unique for the life of the store and DERIVED FROM STORED STATE, including after the
/// delivery holding an id has been deleted.
///
/// AWS documents no format for `Delivery$id` beyond its own constraints (len 1..64, pattern
/// `^[0-9A-Za-z]+$`), so the counter is rendered as a fixed-width hex string, the way
/// [`mint_revision_id`] renders the revision counter.
pub fn mint_delivery_id(seq: u64) -> String {
    format!("{seq:016x}")
}

/// THE EMULATOR'S OWN CEILING on the deliveries this store holds, across every Region, not a
/// quota cited from AWS: each delivery keeps its configuration and tag set in memory for as
/// long as the process runs and nothing expires one, so `CreateDelivery` refuses to create more
/// than this many with the `ServiceQuotaExceededException` it models ("This request exceeds a
/// service quota") rather than growing without bound on request.
pub const MAX_DELIVERIES: usize = 1_000;

/// The refusal CloudWatch Logs answers a delivery operation with when the `id` it names is
/// not one this account holds. Like [`DELIVERY_SOURCE_NOT_FOUND_MESSAGE`], the sentence states
/// that the requested delivery is not in the account and interpolates no id, so every id the
/// account does not hold is refused identically.
pub const DELIVERY_NOT_FOUND_MESSAGE: &str = "Requested Delivery does not exist in this account.";

/// The refusal CloudWatch Logs answers a `PutDeliverySource` with when the resource it names
/// is already represented for that same log type by ANOTHER delivery source of the account
/// ([`crate::state::ComAmazonawsCloudwatchlogsLogs20140328State::delivery_source_representing`]).
///
/// The sentence is FIXED for that error class: it names neither the resource, nor the log type,
/// nor the source that already represents the pairing, so every such put is refused
/// identically, and it points the caller at the resource id it re-used.
pub const DELIVERY_SOURCE_RESOURCE_IN_USE_MESSAGE: &str =
    "This ResourceId has already been used in another Delivery Source in this account.";

/// The refusal the vended-log delivery ENUMERATIONS answer a `nextToken` they cannot resume
/// from with — one this service never minted, one whose payload it cannot decode, and one
/// minted for a different enumeration than the request carrying it describes.
///
/// The delivery-pipeline reads word that class their own way rather than with the sentence the
/// rest of this service's paginated reads word it with ([`INVALID_NEXT_TOKEN_MESSAGE`]): the
/// text states only that the next token is invalid, carrying neither the token nor the
/// operation, so a paginating reader that keys on it restarts from the head of the stream.
pub const INVALID_DELIVERY_NEXT_TOKEN_MESSAGE: &str = "Invalid Next Token.";

/// The refusal CloudWatch Logs answers a delivery operation with when the
/// `deliverySourceName` it names is not one this account holds. Like
/// [`DELIVERY_DESTINATION_NOT_FOUND_MESSAGE`], the sentence states that the requested delivery
/// source is not in the account and interpolates no name, so every name the account does not
/// hold is refused identically.
pub const DELIVERY_SOURCE_NOT_FOUND_MESSAGE: &str =
    "Requested Delivery Source does not exist in this account.";

/// The refusal CloudWatch Logs answers a create with when the calling principal's own
/// policies do not let it make the write that grants the log-delivery service access to the
/// destination resource — the resource policy such a resource of this service's carries.
///
/// The sentence is FIXED for that error class: it names neither the destination, nor the
/// resource behind it, nor the principal, nor the action withheld, and it points the caller
/// at the permissions it needs on the destination resource instead. So every caller that
/// cannot wire a delivery to a destination is refused identically, whichever destination it
/// named.
pub const DELIVERY_DESTINATION_ACCESS_DENIED_MESSAGE: &str =
    "Access Denied for this Delivery Destination. Please make sure that you have correct \
     permissions to access the Log Destination Resource.";

/// The refusal CloudWatch Logs answers the delete of a delivery-pipeline resource with while a
/// delivery still references it — "You can't delete a delivery destination if any current
/// deliveries are associated with it", and the same for a delivery source.
///
/// The sentence is FIXED for that error class apart from the KIND of resource the delete
/// addresses, exactly as [`resource_not_found_message`] is: it interpolates neither the
/// resource's name nor the id of any delivery, and it directs the caller to enumerate the
/// deliveries itself (`DescribeDeliveries`, which the operation's documentation points at)
/// because ANY number of deliveries can reference the resource — a wording that named one of
/// them would promise a caller that deleting that one unblocks the delete.
pub fn delivery_resource_in_use_message(resource_kind: &str) -> String {
    format!(
        "Could not delete this {resource_kind} as it is currently in use. Please delete any \
         Deliveries that reference this resource and try again."
    )
}

/// The resource kind [`delivery_resource_in_use_message`] names for a delivery destination,
/// spelled the way this service's prose spells the resource.
pub const DELIVERY_DESTINATION_RESOURCE_KIND: &str = "Delivery Destination";

/// The resource kind [`delivery_resource_in_use_message`] names for a delivery source.
pub const DELIVERY_SOURCE_RESOURCE_KIND: &str = "Delivery Source";

/// The ARN service namespace of each vendor a delivery destination can be, paired with the
/// `DeliveryDestinationType` the model spells it as.
///
/// The four vendors are the ones `DescribeDeliveries` documents — "The destination can be
/// CloudWatch Logs, Amazon S3, Firehose or X-Ray" — and each spelling is a value of the
/// model's `DeliveryDestinationType` enum
/// ([`crate::ops::types::DELIVERY_DESTINATION_TYPE_VALUES`]), never a string typed here.
pub const DELIVERY_DESTINATION_VENDOR_TYPES: [(&str, &str); 4] = [
    ("logs", "CWL"),
    ("firehose", "FH"),
    ("s3", "S3"),
    ("xray", "XRAY"),
];

/// The `DeliveryDestinationType` of the resource an ARN names, or `None` for an ARN naming
/// none of the four vendors a delivery destination can be.
///
/// The type is a property of the destination RESOURCE, not of the request:
/// `Delivery$deliveryDestinationType` "Displays whether the delivery destination associated
/// with this delivery is CloudWatch Logs, Amazon S3, Firehose, or X-Ray", so it is read off
/// the destination's ARN ([`DELIVERY_DESTINATION_VENDOR_TYPES`]).
pub fn delivery_destination_type_for_arn(arn: &str) -> Option<&'static str> {
    let (vendor, _resource) = arn_service_and_resource(arn)?;
    DELIVERY_DESTINATION_VENDOR_TYPES
        .iter()
        .find(|(namespace, _)| vendor == *namespace)
        .map(|(_, destination_type)| *destination_type)
}

/// The `DeliveryDestinationType` of a TRACE delivery destination: the one value of the
/// model's `DeliveryDestinationType` that `PutDeliveryDestination` documents as needing no
/// `deliveryDestinationConfiguration` ("deliveryDestinationConfiguration is required for
/// CloudWatch Logs, Amazon S3, Firehose log delivery destinations and not required for X-Ray
/// trace delivery destinations"), and the one kind `roleArn` is supported for ("This
/// parameter is supported only for X-Ray trace delivery destinations").
///
/// Spelled as the model's enum spells it
/// ([`crate::ops::types::DELIVERY_DESTINATION_TYPE_VALUES`]), never a string typed here.
pub const TRACE_DELIVERY_DESTINATION_TYPE: &str = "XRAY";

/// The ARN service namespace of CloudWatch Logs itself, the vendor whose
/// `destinationResourceArn` must name a log group.
pub(super) const CLOUDWATCH_LOGS_VENDOR: &str = "logs";

/// THE EMULATOR'S OWN CEILING on the delivery destinations this store holds, across every
/// Region, not a quota cited from AWS: each destination keeps its configuration and tag set in
/// memory for as long as the process runs and nothing expires one, so `PutDeliveryDestination`
/// refuses to CREATE more than this many with the `ServiceQuotaExceededException` it models
/// ("This request exceeds a service quota") rather than growing without bound on request. A put
/// that UPDATES a destination already stored is never refused by it: it adds nothing.
pub const MAX_DELIVERY_DESTINATIONS: usize = 1_000;

/// Judge a `DeliveryDestinationConfiguration$destinationResourceArn`, answering the
/// `DeliveryDestinationType` of the resource it names or the message to refuse it with.
///
/// The resources a delivery destination can represent are the ones the member documents —
/// "That Amazon Web Services destination can be a log group in CloudWatch Logs, an Amazon S3
/// bucket, or a delivery stream in Firehose" — plus the X-Ray trace destination the model's
/// `DeliveryDestinationType` adds, which is exactly
/// [`DELIVERY_DESTINATION_VENDOR_TYPES`]. A CloudWatch Logs ARN qualifies only when it names
/// a LOG GROUP: a log stream or any other `logs` resource is not a destination logs can be
/// delivered to.
///
/// Whether that resource exists is a question this service cannot answer for another
/// service's bucket or stream, and the operation documents no existence requirement, so it
/// is not asked here.
pub fn check_destination_resource_arn(arn: &str) -> Result<&'static str, String> {
    if let Some(message) = unparsable_arn_message("destinationResourceArn", arn) {
        return Err(message);
    }
    let Some(destination_type) = delivery_destination_type_for_arn(arn) else {
        return Err(format!(
            "destinationResourceArn must name a resource of one of: [{}]",
            DELIVERY_DESTINATION_VENDOR_TYPES
                .iter()
                .map(|(vendor, _)| *vendor)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    };
    let names_a_cloudwatch_logs_resource = arn_service_and_resource(arn)
        .is_some_and(|(vendor, _resource)| vendor == CLOUDWATCH_LOGS_VENDOR);
    if names_a_cloudwatch_logs_resource && !is_log_group_arn(arn) {
        return Err(
            "destinationResourceArn for a CloudWatch Logs delivery destination must name a \
             log group"
                .to_string(),
        );
    }
    Ok(destination_type)
}

/// One delivery destination — "a logical object that represents the actual delivery
/// destination", the Amazon Web Services resource logs are sent to — as it is stored. Every
/// member of the model's `DeliveryDestination` shape is reported from this record, so
/// `GetDeliveryDestination` answers from state alone.
#[derive(Debug, Clone)]
pub struct DeliveryDestinationRecord {
    /// `DeliveryDestination$name` — "The name of this delivery destination." Also the name
    /// half of the key the store holds it under ([`RegionalKey`]), because the name "must be
    /// unique for all delivery destinations in your account".
    pub name: String,
    /// `DeliveryDestination$arn` — "The Amazon Resource Name (ARN) that uniquely identifies
    /// this delivery destination", as [`delivery_destination_arn`] builds it.
    pub arn: String,
    /// `DeliveryDestination$deliveryDestinationType` — "Displays whether this delivery
    /// destination is CloudWatch Logs, Amazon S3, Firehose, or X-Ray". `None` only for a
    /// destination created with neither a configuration nor a type, which no accepted put
    /// produces.
    pub delivery_destination_type: Option<String>,
    /// `DeliveryDestination$outputFormat` — "The format of the logs that are sent to this
    /// delivery destination."
    pub output_format: Option<String>,
    /// `DeliveryDestinationConfiguration$destinationResourceArn` — "The ARN of the Amazon
    /// Web Services destination that this delivery destination represents." `None` for a
    /// trace delivery destination, the one kind that needs no configuration.
    pub destination_resource_arn: Option<String>,
    /// `DeliveryDestination$roleArn` — "The ARN of the IAM role that CloudWatch Logs assumes
    /// to deliver to this delivery destination. This field is present only for X-Ray trace
    /// delivery destinations that were created with a role."
    pub role_arn: Option<String>,
    /// `DeliveryDestination$tags` — "The tags that have been assigned to this delivery
    /// destination." A `BTreeMap` so the stored order is stable; `Tags` is `@length(min: 1)`,
    /// so a destination carrying none reports no member at all.
    pub tags: BTreeMap<String, String>,
    /// `Policy$deliveryDestinationPolicy` — "The contents of the delivery destination
    /// policy", the IAM policy `PutDeliveryDestinationPolicy` assigns to this destination,
    /// `GetDeliveryDestinationPolicy` reports and `DeleteDeliveryDestinationPolicy` takes back
    /// off. `None` for a destination no policy has been assigned to, which is what makes a get
    /// on one report no policy at all rather than refuse.
    ///
    /// It is NOT one of the "delivery destination parameters" a later put overwrites: the
    /// policy is assigned by its own operation, so a put that updates the destination's
    /// parameters leaves it where it is.
    ///
    /// Held as the document the assignment read rather than as the bytes the caller rendered:
    /// the whitespace between its JSON tokens is dropped on the way in
    /// ([`service_common::json_document::minified`]), so every reader of this field reports the
    /// same text whichever way the caller spaced it.
    pub policy: Option<String>,
}

impl DeliveryDestinationRecord {
    /// A brand-new delivery destination of `destination_type`, with its ARN DERIVED rather
    /// than taken from the caller: the ARN identifies the logical object this service just
    /// created, so it is built from the name and the request's Region.
    ///
    /// The optional members a put may name — `outputFormat`, the configuration's
    /// `destinationResourceArn`, `roleArn`, `tags` — are left at their "the request named
    /// none" value and set on the returned record by the caller that has them.
    pub fn new(region: &str, name: &str, destination_type: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            arn: delivery_destination_arn(region, name),
            delivery_destination_type: destination_type.map(ToString::to_string),
            output_format: None,
            destination_resource_arn: None,
            role_arn: None,
            tags: BTreeMap::new(),
            policy: None,
        }
    }

    /// Whether this destination is a TRACE delivery destination
    /// ([`TRACE_DELIVERY_DESTINATION_TYPE`]) — the one kind that carries no destination
    /// configuration and the one kind `roleArn` applies to.
    pub fn is_trace_destination(&self) -> bool {
        self.delivery_destination_type.as_deref() == Some(TRACE_DELIVERY_DESTINATION_TYPE)
    }

    /// This destination as the model's `DeliveryDestination` structure, which is what
    /// `PutDeliveryDestination`, `GetDeliveryDestination` and `DescribeDeliveryDestinations`
    /// all report, so one record renders the same way through any of them.
    pub fn reported(&self) -> crate::ops::types::DeliveryDestination {
        crate::ops::types::DeliveryDestination {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            delivery_destination_type: self.delivery_destination_type.clone(),
            output_format: self.output_format.clone(),
            delivery_destination_configuration: self.destination_resource_arn.clone().map(
                |destination_resource_arn| crate::ops::types::DeliveryDestinationConfiguration {
                    destination_resource_arn,
                },
            ),
            role_arn: self.role_arn.clone(),
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
        }
    }
}

/// A delivery destination's ARN, the form `DeliveryDestination$arn` ("The Amazon Resource
/// Name (ARN) that uniquely identifies this delivery destination") carries: a
/// `delivery-destination` resource of this service, identified by the destination's own
/// name.
pub fn delivery_destination_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("delivery-destination:{name}"))
}

/// THE EMULATOR'S OWN CEILING on the delivery sources this store holds, across every Region,
/// not a quota cited from AWS: each source keeps its configuration and tag set in memory for as
/// long as the process runs and nothing expires one, so `PutDeliverySource` refuses to CREATE
/// more than this many with the `ServiceQuotaExceededException` it models ("This request
/// exceeds a service quota") rather than growing without bound on request. A put that UPDATES a
/// source already stored is never refused by it: it adds nothing.
pub const MAX_DELIVERY_SOURCES: usize = 1_000;

/// One delivery source — "a logical object that represents the resource that is actually
/// sending the logs" — as it is stored. Every member of the model's `DeliverySource` shape is
/// reported from this record, so `GetDeliverySource` answers from state alone.
#[derive(Debug, Clone)]
pub struct DeliverySourceRecord {
    /// `DeliverySource$name` — "The unique name of the delivery source." Also the name half
    /// of the key the store holds it under ([`RegionalKey`]), because the name "must be unique
    /// for all delivery sources in your account".
    pub name: String,
    /// `DeliverySource$arn` — "The Amazon Resource Name (ARN) that uniquely identifies this
    /// delivery source", as [`delivery_source_arn`] builds it.
    pub arn: String,
    /// `DeliverySource$resourceArns` — "This array contains the ARN of the Amazon Web Services
    /// resource that sends logs and is represented by this delivery source. Currently, only
    /// one ARN can be in the array."
    pub resource_arns: Vec<String>,
    /// `DeliverySource$service` — "The Amazon Web Services service that is sending logs",
    /// DERIVED from the resource that sends them ([`delivery_source_service`]) rather than
    /// taken from the caller, who never states it: `PutDeliverySource` has no `service`
    /// member. `None` for a resource ARN naming no service namespace.
    pub service: Option<String>,
    /// `DeliverySource$logType` — "The type of log that the source is sending."
    pub log_type: String,
    /// `DeliverySource$deliverySourceConfiguration` — "The map of key-value pairs that
    /// configure the delivery source." A `BTreeMap` so the stored order is stable; the
    /// modelled map has no minimum length, so a source configured with none reports no member
    /// at all.
    pub source_configuration: BTreeMap<String, String>,
    /// `DeliverySource$tags` — "The tags that have been assigned to this delivery source." A
    /// `BTreeMap` so the stored order is stable; `Tags` is `@length(min: 1)`, so a source
    /// carrying none reports no member at all.
    pub tags: BTreeMap<String, String>,
}

impl DeliverySourceRecord {
    /// A brand-new delivery source for one resource, with its ARN and its `service` DERIVED
    /// rather than taken from the caller: the ARN identifies the logical object this service
    /// just created, and the service sending the logs is a fact about the resource named.
    ///
    /// The optional members a put may name — the configuration map and `tags` — are left at
    /// their "the request named none" value and set on the returned record by the caller that
    /// has them.
    pub fn new(region: &str, name: &str, resource_arn: &str, log_type: &str) -> Self {
        Self {
            name: name.to_string(),
            arn: delivery_source_arn(region, name),
            resource_arns: vec![resource_arn.to_string()],
            service: delivery_source_service(resource_arn).map(ToString::to_string),
            log_type: log_type.to_string(),
            source_configuration: BTreeMap::new(),
            tags: BTreeMap::new(),
        }
    }

    /// This source as the model's `DeliverySource` structure, which is what
    /// `PutDeliverySource`, `GetDeliverySource` and `DescribeDeliverySources` all report, so
    /// one record renders the same way through any of them.
    ///
    /// `status` and `statusReason` are left unreported: the model documents both as "defined
    /// for selective log types", and nothing in this store makes the resource behind a source
    /// disappear (the one `statusReason` the model enumerates says the resource "has been
    /// deleted"), so there is no state to report them from.
    pub fn reported(&self) -> crate::ops::types::DeliverySource {
        crate::ops::types::DeliverySource {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            resource_arns: Some(self.resource_arns.clone()),
            service: self.service.clone(),
            log_type: Some(self.log_type.clone()),
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
            delivery_source_configuration: (!self.source_configuration.is_empty()).then(|| {
                self.source_configuration
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            }),
            status: None,
            status_reason: None,
        }
    }
}

/// A delivery source's ARN, the form `DeliverySource$arn` ("The Amazon Resource Name (ARN)
/// that uniquely identifies this delivery source") carries: a `delivery-source` resource of
/// this service, identified by the source's own name.
pub fn delivery_source_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("delivery-source:{name}"))
}

/// The Amazon Web Services service that sends a delivery source's logs: the service
/// namespace of the resource ARN the source represents, or `None` for a value that is not an
/// ARN or names no namespace.
///
/// `DeliverySource$service` is "The Amazon Web Services service that is sending logs" and no
/// input member states it, so it is read off `resourceArn` — "The ARN of the Amazon Web
/// Services resource that is generating and sending logs" — exactly as a delivery reads its
/// destination's type off the destination ARN ([`delivery_destination_type_for_arn`]).
pub fn delivery_source_service(resource_arn: &str) -> Option<&str> {
    arn_service_and_resource(resource_arn)
        .map(|(service, _resource)| service)
        .filter(|service| !service.is_empty())
}

/// The `logType` of a delivery source that sends TRACES rather than log records, as the
/// `logType` member's own catalogue spells it ("For Amazon Bedrock AgentCore Runtime, the
/// valid values are `APPLICATION_LOGS`, `USAGE_LOGS`, and `TRACES`"). Source: the `logType`
/// member of `PutDeliverySourceRequest`
/// (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`).
///
/// It is the one log type delivered to an X-Ray TRACE delivery destination
/// ([`TRACE_DELIVERY_DESTINATION_TYPE`]) rather than to a destination that receives log
/// records.
pub const TRACE_LOG_TYPE: &str = "TRACES";

/// The `DeliveryDestinationType` of a delivery destination that is a log group of this
/// service, as the model's enum spells it (the `logs` vendor of
/// [`DELIVERY_DESTINATION_VENDOR_TYPES`]).
pub const CWL_DELIVERY_DESTINATION_TYPE: &str = "CWL";

/// The `DeliveryDestinationType` of a delivery destination that is an Amazon S3 bucket, as
/// the model's enum spells it (the `s3` vendor of [`DELIVERY_DESTINATION_VENDOR_TYPES`]).
pub const S3_DELIVERY_DESTINATION_TYPE: &str = "S3";

/// The `DeliveryDestinationType` of a delivery destination that receives a STREAM of records,
/// as the model's enum spells it (the `firehose` vendor of
/// [`DELIVERY_DESTINATION_VENDOR_TYPES`]).
pub const STREAM_DELIVERY_DESTINATION_TYPE: &str = "FH";

/// The one `OutputFormat` that carries a whole record on its own, needing neither a field
/// order nor a delimiter to be read back. Spelled as the model's `OutputFormat` enum spells
/// it ([`crate::ops::types::OUTPUT_FORMAT_VALUES`]).
pub const STRUCTURED_OUTPUT_FORMAT: &str = "json";

/// The one `OutputFormat` that is a columnar FILE layout rather than a per-record encoding,
/// so it applies only where a delivery writes objects. Spelled as the model's `OutputFormat`
/// enum spells it ([`crate::ops::types::OUTPUT_FORMAT_VALUES`]).
pub const COLUMNAR_OUTPUT_FORMAT: &str = "parquet";

/// The `OutputFormat`s whose records are field values joined by a delimiter — the formats
/// `fieldDelimiter` is about ("The field delimiter that is used between record fields when
/// the final output format of a delivery is in Plain, W3C, or Raw format"). Each is spelled
/// as the model's `OutputFormat` enum spells it
/// ([`crate::ops::types::OUTPUT_FORMAT_VALUES`]).
pub const DELIMITED_OUTPUT_FORMATS: [&str; 3] = ["plain", "raw", "w3c"];
