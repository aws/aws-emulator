// SPDX-License-Identifier: CC0-1.0

//! The SYSTEMS MANAGER OPSITEM actions an alarm may carry, and what executing one does.
//!
//! "Valid Values: […] Systems Manager actions:
//! arn:aws:ssm:region:account-id:opsitem:severity#CATEGORY=category_name"
//! (`PutMetricAlarmInput$AlarmActions`, and the same for `OKActions`,
//! `InsufficientDataActions` and every action list of `PutCompositeAlarm`) — so an alarm whose
//! transition fires one of these ARNs has Systems Manager OpsCenter OPEN AN OPSITEM for that
//! transition: a work item at the SEVERITY the ARN names, in the CATEGORY it names when it
//! names one, naming the alarm that raised it.
//!
//! WHAT the OpsItem says is read off the transition and nothing else: its title is the alarm's
//! own name, its origin is the fixed name OpsCenter records for an alarm-raised work item
//! ([`OPS_ITEM_SOURCE`]), its description is the state the alarm moved to beside the reason the
//! alarm gave for moving ([`ops_item_description`]), and the alarm itself is the RELATED
//! RESOURCE the item carries under the service-defined operational-data key for one
//! ([`related_resources`]). The severity and the category are the action's own, taken from the
//! ARN rather than chosen here, so an ARN naming a severity or a category Systems Manager does
//! not support is refused BY that service and the refusal is what the alarm's history records
//! — nothing is invented for it and no OpsItem is reported that OpsCenter does not hold.
//!
//! Systems Manager is reached the way any other service is reached from here: over the AWS SDK
//! driven on the in-process callback ([`smithy_server_runtime::OperationContext::callback`]),
//! through this workspace's one construction of that hop
//! ([`::service_common::ssm_client`]), in the Region the action's own ARN names — which is the
//! alarm's own Region, since an action naming another one is refused when the alarm is written.
//!
//! No authorization document is read for this hop, where a topic action reads the topic's policy
//! ([`super::publish_refusal`]) and a function action the function's
//! ([`super::lambda_actions`]): an OpsItem is not a resource that carries a policy of its own,
//! so there is no document for OpsCenter to decide the create by. What admits the action on AWS
//! is the service-linked role `PutMetricAlarm` documents for it, which is an identity rather
//! than a document this hop can read.

use ::serde_json::Value;

/// The ARN vendor of a Systems Manager OpsItem action, which is Systems Manager's own ARN
/// namespace (`aws.api#service$arnNamespace` of `com.amazonaws.ssm#AmazonSSM`).
const SSM_ARN_VENDOR: &str = "ssm";

/// The ARN resource type of the action, which is what tells it from every other ARN of this
/// vendor (a parameter, a document, an OpsItem group — and an `opsitem/<id>`, which names one
/// OpsItem that already exists rather than the action that opens one).
const OPS_ITEM_RESOURCE_TYPE: &str = "opsitem";

/// What the CATEGORY is appended to the severity with: `…:opsitem:<severity>#CATEGORY=<name>`
/// (`PutMetricAlarmInput$AlarmActions`).
const CATEGORY_MARKER: &str = "#CATEGORY=";

/// "The origin of the OpsItem" (`CreateOpsItemRequest$Source`) for every work item an alarm
/// action opens: the one name OpsCenter records such an item's origin as, whichever alarm
/// raised it (recorded behaviour of this action — WHICH alarm raised it is the item's title and
/// its description, not its origin).
const OPS_ITEM_SOURCE: &str = "CloudWatch Alarm";

/// The modelled maximum length of `CreateOpsItemRequest$Description` (`OpsItemDescription`,
/// `@length 1..2048`), which a state reason (`@length 0..1023`) beside an alarm's name need not
/// fit.
const MAX_OPS_ITEM_DESCRIPTION: usize = 2048;

/// The operational-data key an OpsItem carries a related resource under: "use the
/// `/aws/resources` key in `OperationalData` to specify a related resource in the request"
/// (`CreateOpsItemRequest$OperationalData`).
const RELATED_RESOURCES_KEY: &str = "/aws/resources";

/// The member each entry of that key's document names a resource by, as the documented example
/// of the key writes it (`[{"arn": "<resource arn>"}]`).
const RELATED_RESOURCE_ARN_MEMBER: &str = "arn";

/// The `OpsItemDataType` an operational-data value this action writes carries, which is the one
/// value the modelled enum declares.
const SEARCHABLE_STRING: ::aws_sdk_ssm::types::OpsItemDataType =
    ::aws_sdk_ssm::types::OpsItemDataType::SearchableString;

/// One OpsItem action an alarm action list carries: where it is performed, and what the OpsItem
/// it opens carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpsItemAction<'arn> {
    /// The ARN's own region field, which is where the OpsItem is opened.
    region: &'arn str,
    /// The severity the OpsItem carries, as the ARN spells it. Held to no value set here:
    /// which severities exist is Systems Manager's to say, and it says so by refusing the
    /// create.
    severity: &'arn str,
    /// The category the OpsItem carries, when the ARN names one.
    category: Option<&'arn str>,
}

/// The OpsItem action `arn` names, or `None` when it names none.
///
/// The partition is not matched: an OpsItem action is one whatever partition it names. Neither
/// is the Region, which is where the action is performed rather than part of what makes the ARN
/// an OpsItem action's (the alarm writers refuse an action naming another Region for that
/// reason).
#[must_use]
pub fn of_action_arn(arn: &str) -> Option<OpsItemAction<'_>> {
    let parsed = ::service_common::arn::parse(arn)?;
    if parsed.vendor != SSM_ARN_VENDOR {
        return None;
    }
    // `opsitem:<severity>…`: the SEVERITY is the resource id of that type, and an
    // `opsitem/<id>` names an OpsItem rather than this action, so only the `:` form is read.
    let rest = parsed
        .resource
        .strip_prefix(OPS_ITEM_RESOURCE_TYPE)?
        .strip_prefix(':')?;
    let (severity, category) = match rest.split_once(CATEGORY_MARKER) {
        Some((severity, category)) => (severity, Some(category)),
        None => (rest, None),
    };
    (!severity.is_empty()).then_some(OpsItemAction {
        region: parsed.region,
        severity,
        // A marker carrying no name after it names no category, so none is sent rather than an
        // empty one, which the member's own `@pattern` would refuse.
        category: category.filter(|category| !category.is_empty()),
    })
}

/// Open the OpsItem the action `arn` names for `notification`'s transition, and answer the
/// failure it failed with — `None` when Systems Manager opened one.
///
/// Nothing is invented for an action that could not be carried out: what Systems Manager said
/// is what the alarm's history records ([`super::action_history_item`]), so an OpsItem the
/// emulator reports as opened is one OpsCenter's own reads hold.
pub async fn execute(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    arn: &str,
    notification: &super::AlarmNotification,
) -> Option<String> {
    let action = of_action_arn(arn)?;
    // The Region ends up in the endpoint URL this hop is addressed on, so it is read under the
    // Region grammar every model states for the field rather than as arbitrary text.
    let Some(region) = ::service_common::arn::region_label(action.region) else {
        return Some(format!(
            "The action {arn} names no Region in which an OpsItem can be created"
        ));
    };
    let mut create = ::service_common::ssm_client::ssm_client(callback, region)
        .create_ops_item()
        .title(notification.alarm_name.as_str())
        .description(ops_item_description(notification))
        .source(OPS_ITEM_SOURCE)
        .severity(action.severity)
        .set_category(action.category.map(str::to_string));
    // The alarm is the resource this work item is ABOUT, which is what the related-resource
    // key carries. An alarm whose own ARN this store does not hold names no resource, so the
    // key is left off rather than written empty.
    if let Some(alarm_arn) = notification.alarm_arn.as_deref() {
        create = create.operational_data(
            RELATED_RESOURCES_KEY,
            ::aws_sdk_ssm::types::OpsItemDataValue::builder()
                .value(related_resources(alarm_arn))
                .r#type(SEARCHABLE_STRING)
                .build(),
        );
    }
    create
        .send()
        .await
        .err()
        .map(|error| super::action_failure(&error))
}

/// The document the related-resource key carries: the resources the OpsItem is about, each
/// named by its ARN, in the form the key's own documented example writes
/// (`[{"arn": "<resource arn>"}]`). One resource here — the alarm whose transition opened it.
fn related_resources(alarm_arn: &str) -> String {
    Value::Array(vec![Value::Object(
        [(
            RELATED_RESOURCE_ARN_MEMBER.to_string(),
            Value::from(alarm_arn),
        )]
        .into_iter()
        .collect(),
    )])
    .to_string()
}

/// "User-defined text that contains information about the OpsItem"
/// (`CreateOpsItemRequest$Description`): the state the alarm moved to, and the reason the alarm
/// gave for moving when it gave one.
fn ops_item_description(notification: &super::AlarmNotification) -> String {
    let mut description = format!(
        "The CloudWatch alarm {} transitioned to the {} state",
        notification.alarm_name, notification.new_state_value
    );
    if let Some(reason) = notification
        .state_reason
        .as_deref()
        .filter(|reason| !reason.trim().is_empty())
    {
        description.push_str(": ");
        description.push_str(reason);
    }
    super::bounded_to(description, MAX_OPS_ITEM_DESCRIPTION)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which ARNs name an OpsItem action, and what each one says the OpsItem carries.
    #[test]
    fn an_ops_item_action_is_told_from_every_other_action_kind() {
        for (case, arn, expected) in [
            (
                "a severity alone",
                "arn:aws:ssm:us-west-2:000000000000:opsitem:3",
                Some(("us-west-2", "3", None)),
            ),
            (
                "a severity and a category",
                "arn:aws:ssm:us-west-2:000000000000:opsitem:2#CATEGORY=Availability",
                Some(("us-west-2", "2", Some("Availability"))),
            ),
            (
                "another partition and another Region",
                "arn:aws-us-gov:ssm:us-gov-west-1:000000000000:opsitem:1#CATEGORY=Security",
                Some(("us-gov-west-1", "1", Some("Security"))),
            ),
            (
                "a marker carrying no category name",
                "arn:aws:ssm:us-west-2:000000000000:opsitem:4#CATEGORY=",
                Some(("us-west-2", "4", None)),
            ),
            (
                "an OpsItem that already exists, not the action that opens one",
                "arn:aws:ssm:us-west-2:000000000000:opsitem/oi-0123456789ab",
                None,
            ),
            (
                "another resource of the same vendor",
                "arn:aws:ssm:us-west-2:000000000000:parameter/fleet/size",
                None,
            ),
            (
                "an OpsItem group of the same vendor",
                "arn:aws:ssm:us-west-2:000000000000:opsitemgroup/default",
                None,
            ),
            (
                "a response plan of another vendor",
                "arn:aws:ssm-incidents::000000000000:responseplan/outage",
                None,
            ),
            (
                "an SNS topic",
                "arn:aws:sns:us-west-2:000000000000:alarms",
                None,
            ),
            ("no severity at all", "arn:aws:ssm:us-west-2:000000000000:opsitem:", None),
            ("not an ARN at all", "opsitem:3", None),
        ] {
            let found = of_action_arn(arn)
                .map(|action| (action.region, action.severity, action.category));
            assert_eq!(
                found, expected,
                "case {case:?}: expected {expected:?} for {arn:?}, got {found:?}"
            );
        }
    }
}
