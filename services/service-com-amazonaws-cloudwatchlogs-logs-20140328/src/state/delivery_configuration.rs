// SPDX-License-Identifier: CC0-1.0

//! The EFFECTIVE configuration of a delivery: which configuration template governs it, what
//! that template defaults the delivery's configuration members to, and whether the delivery
//! source it is made from supports a record-field selection at all.
//!
//! A delivery's configuration is "the valid and default values that are used when creating
//! deliveries" of one log type to one kind of destination — which is exactly what a
//! configuration template publishes
//! ([`super::vended_log_templates`]) — so the template that governs a delivery is the one
//! keyed by the delivery's own source and destination, and the members the create does not
//! state take their values from it rather than being left unreported.

use super::*;

/// The OBJECT LAYOUT the catalogue publishes for the logs `source` sends: the
/// `defaultDeliveryConfigValues.s3DeliveryConfiguration` of the template for delivering them to
/// an S3 BUCKET, the one kind of destination an object layout is published for.
///
/// An object layout is published per LOG TYPE — `S3DeliveryConfiguration$suffixPath` documents
/// that "the valid variables to use in the suffix path vary by log type" and that a caller finds
/// them through `DescribeConfigurationTemplates` — so it is a fact about the SENDER, not about
/// the kind of destination a particular delivery pairs it with. It is therefore the layout EVERY
/// delivery of those logs reports, whatever it is pointed at: a delivery to a destination that
/// is not a bucket reports the same layout as one to a bucket rather than losing it, which is
/// what lets a caller read back where its objects would land.
///
/// `None` where the catalogue publishes no template for these logs at all: nothing published
/// says what layout they are delivered under.
pub fn delivery_object_layout(
    source: &DeliverySourceRecord,
) -> Option<crate::ops::types::S3DeliveryConfiguration> {
    delivery_configuration_template(source, S3_DELIVERY_DESTINATION_TYPE)?
        .default_delivery_config_values?
        .s3_delivery_configuration
}

/// The configuration template that governs a delivery between `source` and a destination of
/// `delivery_destination_type`, or `None` when the catalogue publishes none for them.
///
/// A template covers delivering ONE log type of ONE kind of resource to ONE kind of
/// destination, so it is found by the four values that identify it: the service sending the
/// logs and the log type it sends — both facts about the delivery SOURCE — the kind of
/// resource sending them, and the kind of destination receiving them.
///
/// The resource KIND is read off the source's own resource ARN
/// ([`resource_arn_names_resource_type`]) because `DeliverySource` has no member stating it.
/// Where the catalogue publishes exactly ONE row for that sender and log type, that row is the
/// template even when the ARN spells its resource some other way: the sender and the log type
/// already identify it, and a template that applies to no resource type of the service would be
/// no template at all. Where it publishes SEVERAL — one per kind of resource that sends those
/// logs — the one the ARN names is the delivery's, and an ARN naming none of them is governed by
/// no published template rather than by whichever row comes first.
pub fn delivery_configuration_template(
    source: &DeliverySourceRecord,
    delivery_destination_type: &str,
) -> Option<crate::ops::types::ConfigurationTemplate> {
    let service = source.service.as_deref()?;
    let published: Vec<crate::ops::types::ConfigurationTemplate> = configuration_templates()
        .into_iter()
        .filter(|template| {
            template.service.as_deref() == Some(service)
                && template.log_type.as_deref() == Some(source.log_type.as_str())
                && template.delivery_destination_type.as_deref() == Some(delivery_destination_type)
        })
        .collect();
    published
        .iter()
        .find(|template| {
            let resource_type = template.resource_type.as_deref().unwrap_or_default();
            source
                .resource_arns
                .iter()
                .any(|resource_arn| resource_arn_names_resource_type(resource_arn, resource_type))
        })
        .or_else(|| published.first().filter(|_| published.len() == 1))
        .cloned()
}

/// Whether `resource_arn` names a resource of `resource_type`: the type is one of the segments
/// of the ARN's resource part.
///
/// An ARN's resource part spells its type in its own way — `hubv2/*`, `log-group:<name>`,
/// `/restapis/<id>/stages/<stage>` — so the type is looked for among the segments the two
/// separators an ARN uses (`:` and `/`) divide that part into, rather than assumed to be its
/// head. An empty type names no resource.
pub fn resource_arn_names_resource_type(resource_arn: &str, resource_type: &str) -> bool {
    if resource_type.is_empty() {
        return false;
    }
    arn_service_and_resource(resource_arn).is_some_and(|(_service, resource)| {
        resource
            .split([':', '/'])
            .any(|segment| segment == resource_type)
    })
}

/// The refusal CloudWatch Logs answers a delivery operation that states `recordFields` with
/// when the delivery source it is made from does not support selecting them
/// ([`record_fields_selection_unsupported`]).
///
/// The sentence is FIXED for that error class: it names neither the source, nor the fields
/// asked for, nor the fields the source publishes, and it attributes the refusal to the
/// delivery source instead — so every unsupported selection is refused identically, whichever
/// fields it named.
pub const RECORD_FIELDS_SELECTION_UNSUPPORTED_MESSAGE: &str =
    "Record fields selection is not supported by this Delivery Source.";

/// Whether a delivery governed by `template` may NOT be given a `recordFields` selection.
///
/// `recordFields` is "the list of record fields to be delivered to the destination, in order.
/// If the delivery's log source has mandatory fields, they must be included in this list", so
/// a selection is the caller LEAVING OUT fields the log type carries. A log type whose every
/// published field is mandatory has none that may be left out, so there is no selection to
/// make and one is refused rather than stored.
///
/// A delivery the catalogue publishes no template for is not refused: nothing published says
/// what its records carry, so there is no basis to judge a selection of them.
pub fn record_fields_selection_unsupported(
    template: Option<&crate::ops::types::ConfigurationTemplate>,
) -> bool {
    template
        .and_then(|template| template.allowed_fields.as_ref())
        .is_some_and(|allowed| {
            !allowed.is_empty()
                && allowed
                    .iter()
                    .all(|field| field.mandatory == Some(true))
        })
}
