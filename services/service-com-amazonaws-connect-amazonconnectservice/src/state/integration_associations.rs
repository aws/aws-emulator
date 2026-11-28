// SPDX-License-Identifier: CC0-1.0

//! The Amazon Web Services resources associated with an instance: the record
//! `CreateIntegrationAssociation` writes onto an instance, the modelled summary a reader
//! projects it to, and the rules a request to associate a resource is held to.
//!
//! One concern, so one file: an integration association is a resource of its own that
//! hangs off an instance, and the projection, the ARN it carries and the per-type
//! requirements of its request are needed by every operation of that resource — the
//! create, the list and the delete — so they are written once here rather than in one
//! operation's file.
//!
//! What this module does NOT decide is whether the resource an association POINTS AT
//! exists, or what associating it takes in the service that owns it: that is the
//! integration TARGET, and it lives in [`super::integration_target`] beside the hop into
//! that service.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{InvalidRequestException, IntegrationAssociationSummary};

use super::{tags_out, to_date_time, InstanceRecord};

/// How many integration associations one instance holds before `CreateIntegrationAssociation`
/// refuses another. The emulator's own bound on its store, for the reason
/// [`super::INSTANCE_QUOTA`] gives: a record is kept for the life of the process unless a
/// caller deletes it, so an unbounded create would let any caller that reaches this
/// operation grow the map until the host ran out of memory.
pub const INTEGRATION_ASSOCIATIONS_PER_INSTANCE_QUOTA: usize = 1_000;

/// The `IntegrationType` whose request carries the external application's own members, from
/// `com.amazonaws.connect#IntegrationType`
/// (`smithy_query(query="shape",shape="IntegrationType")`).
pub const EVENT_INTEGRATION_TYPE: &str = "EVENT";

/// The member names this operation publishes for the integration's own members, spelled as
/// the model spells them
/// (`smithy_query(query="operation",operation="CreateIntegrationAssociation")`), for a
/// refusal that names the member it is about.
pub const INTEGRATION_TYPE_PARAMETER: &str = "IntegrationType";
/// See [`INTEGRATION_TYPE_PARAMETER`].
pub const SOURCE_TYPE_PARAMETER: &str = "SourceType";
/// See [`INTEGRATION_TYPE_PARAMETER`].
pub const SOURCE_APPLICATION_NAME_PARAMETER: &str = "SourceApplicationName";
/// See [`INTEGRATION_TYPE_PARAMETER`].
pub const SOURCE_APPLICATION_URL_PARAMETER: &str = "SourceApplicationUrl";

/// The resource-type segment the ARN of an integration association carries, as Amazon
/// Connect writes it: the instance's own ARN, then this segment and the association's id.
const INTEGRATION_ASSOCIATION_SEGMENT: &str = "integration-association";

/// `<instance ARN>/integration-association/<id>`: the ARN an association carries, derived
/// from the instance it hangs off and its own identifier, the way every other resource
/// inside an instance derives its own.
pub fn integration_association_arn(instance_arn: &str, association_id: &str) -> String {
    format!("{instance_arn}/{INTEGRATION_ASSOCIATION_SEGMENT}/{association_id}")
}

/// Which member an `EVENT` integration's request left out, or `None` when the request
/// carries everything the type it names requires.
///
/// `SourceApplicationName`, `SourceApplicationUrl` and `SourceType` are each documented as
/// "only required for the EVENT integration type"
/// (`smithy_query(query="doc",shape="CreateIntegrationAssociationRequest",member="SourceApplicationName")`),
/// so the requirement is a rule of the TYPE rather than of the member: for that one type
/// all three have to be there, and for every other type none of them does. The members are
/// checked in the order the model lists them, so one request earns one refusal.
pub fn missing_event_member(
    integration_type: &str,
    source_application_url: Option<&str>,
    source_application_name: Option<&str>,
    source_type: Option<&str>,
) -> Option<&'static str> {
    if integration_type != EVENT_INTEGRATION_TYPE {
        return None;
    }
    [
        (SOURCE_APPLICATION_URL_PARAMETER, source_application_url),
        (SOURCE_APPLICATION_NAME_PARAMETER, source_application_name),
        (SOURCE_TYPE_PARAMETER, source_type),
    ]
    .into_iter()
    .find_map(|(member, value)| {
        value
            .filter(|value| !value.trim().is_empty())
            .is_none()
            .then_some(member)
    })
}

/// Connect's refusal for a request that leaves out a member the `IntegrationType` it names
/// requires: the modelled `InvalidRequestException` (HTTP 400) naming that member and that
/// type.
///
/// The sentence states the rule the model documents ([`missing_event_member`]); its two
/// variable parts are the member the request left out and the integration type it asked
/// for, both spelled as the model spells them.
pub fn member_required_for_integration_type(
    member: &str,
    integration_type: &str,
) -> InvalidRequestException {
    InvalidRequestException {
        message: Some(format!(
            "{member} is required for the {integration_type} integration type"
        )),
        reason: None,
    }
}

/// One stored integration association: exactly what the create described, plus the
/// identifier and ARN this service minted for it and when and where it was last written.
#[derive(Debug, Clone)]
pub struct IntegrationAssociationRecord {
    /// The `IntegrationAssociationId` this service minted, which the record is keyed by.
    pub id: String,
    /// The association's own ARN ([`integration_association_arn`]).
    pub arn: String,
    /// The instance the association hangs off, as the summary reports it.
    pub instance_id: String,
    /// A member of the modelled `IntegrationType` catalog.
    pub integration_type: String,
    /// The ARN of the resource that was associated.
    pub integration_arn: String,
    /// The external application's name, for the type that carries one.
    pub source_application_name: Option<String>,
    /// The external application's URL, for the type that carries one.
    pub source_application_url: Option<String>,
    /// The data source's type, for the type that carries one.
    pub source_type: Option<String>,
    /// The tags the create stored on the association, which the tagging plane reads and
    /// writes through the association's ARN (`super::tagging`).
    pub tags: BTreeMap<String, String>,
    /// When the record was last written.
    pub last_modified_time: SystemTime,
    /// The Region the call that last wrote it was signed for.
    pub last_modified_region: String,
}

impl IntegrationAssociationRecord {
    /// The modelled `IntegrationAssociationSummary` shape `ListIntegrationAssociations`
    /// reports, projected from the stored record.
    pub fn to_summary(&self) -> IntegrationAssociationSummary {
        IntegrationAssociationSummary {
            integration_association_id: Some(self.id.clone()),
            integration_association_arn: Some(self.arn.clone()),
            instance_id: Some(self.instance_id.clone()),
            integration_type: Some(self.integration_type.clone()),
            integration_arn: Some(self.integration_arn.clone()),
            source_application_url: self.source_application_url.clone(),
            source_application_name: self.source_application_name.clone(),
            source_type: self.source_type.clone(),
            last_modified_time: Some(to_date_time(self.last_modified_time)),
            last_modified_region: Some(self.last_modified_region.clone()),
        }
    }

    /// The tags this association carries, in the form a reader reports them
    /// ([`tags_out`]).
    pub fn reported_tags(&self) -> Option<::std::collections::HashMap<String, String>> {
        tags_out(&self.tags)
    }
}

impl InstanceRecord {
    /// This instance's association of the resource `integration_arn` under
    /// `integration_type`, if it has one.
    ///
    /// One resource is associated to an instance once per integration type, which is what
    /// makes a second create of the same pair a duplicate rather than a second
    /// association.
    pub fn integration_association_of(
        &self,
        integration_type: &str,
        integration_arn: &str,
    ) -> Option<&IntegrationAssociationRecord> {
        self.integration_associations.values().find(|association| {
            association.integration_type == integration_type
                && association.integration_arn == integration_arn
        })
    }

    /// This instance's integration association carrying the ARN `association_arn`, if it
    /// has one — the lookup the tagging plane resolves an association's ARN through.
    pub fn integration_association_by_arn(
        &self,
        association_arn: &str,
    ) -> Option<&IntegrationAssociationRecord> {
        self.integration_associations
            .values()
            .find(|association| association.arn == association_arn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ARN of an association is its instance's ARN plus the association segment and
    /// the association's own identifier, whatever Region or instance the ARN names.
    #[test]
    fn an_association_arn_hangs_off_its_instances_arn() {
        for (region, instance, id) in [
            ("us-east-1", "11111111-2222-3333-4444-555555555555", "a1"),
            ("eu-west-2", "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", "b2"),
        ] {
            let instance_arn = super::super::instance_arn(region, instance);
            let expected = format!("{instance_arn}/integration-association/{id}");
            let actual = integration_association_arn(&instance_arn, id);
            assert_eq!(
                actual, expected,
                "case {region}/{instance}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// One request's integration type and the three members of an external application it
    /// carries, with the member the rule must name as missing.
    #[derive(Debug, Clone, Copy)]
    struct Row {
        case: &'static str,
        integration_type: &'static str,
        url: Option<&'static str>,
        name: Option<&'static str>,
        source_type: Option<&'static str>,
        missing: Option<&'static str>,
    }

    /// The three members of an external application are required for the `EVENT` type and
    /// for no other, and the member a refusal names is the first one the model lists that
    /// the request left out.
    #[test]
    fn the_event_type_requires_the_external_applications_members() {
        const URL: Option<&str> = Some("https://example.com/app");
        const NAME: Option<&str> = Some("app");
        const SOURCE: Option<&str> = Some("SALESFORCE");
        let rows = [
            Row {
                case: "EVENT, all three",
                integration_type: "EVENT",
                url: URL,
                name: NAME,
                source_type: SOURCE,
                missing: None,
            },
            Row {
                case: "EVENT, no url",
                integration_type: "EVENT",
                url: None,
                name: NAME,
                source_type: SOURCE,
                missing: Some(SOURCE_APPLICATION_URL_PARAMETER),
            },
            Row {
                case: "EVENT, no name",
                integration_type: "EVENT",
                url: URL,
                name: None,
                source_type: SOURCE,
                missing: Some(SOURCE_APPLICATION_NAME_PARAMETER),
            },
            Row {
                case: "EVENT, no source type",
                integration_type: "EVENT",
                url: URL,
                name: NAME,
                source_type: None,
                missing: Some(SOURCE_TYPE_PARAMETER),
            },
            Row {
                case: "EVENT, a blank name",
                integration_type: "EVENT",
                url: URL,
                name: Some("   "),
                source_type: SOURCE,
                missing: Some(SOURCE_APPLICATION_NAME_PARAMETER),
            },
            Row {
                case: "EVENT, none of them",
                integration_type: "EVENT",
                url: None,
                name: None,
                source_type: None,
                missing: Some(SOURCE_APPLICATION_URL_PARAMETER),
            },
            Row {
                case: "VOICE_ID, none of them",
                integration_type: "VOICE_ID",
                url: None,
                name: None,
                source_type: None,
                missing: None,
            },
            Row {
                case: "MESSAGE_PROCESSOR, none of them",
                integration_type: "MESSAGE_PROCESSOR",
                url: None,
                name: None,
                source_type: None,
                missing: None,
            },
            Row {
                case: "CASES_DOMAIN, all three",
                integration_type: "CASES_DOMAIN",
                url: URL,
                name: NAME,
                source_type: SOURCE,
                missing: None,
            },
        ];
        for row in rows {
            let actual = missing_event_member(
                row.integration_type,
                row.url,
                row.name,
                row.source_type,
            );
            assert_eq!(
                actual, row.missing,
                "case {:?}: expected {:?}, got {actual:?}",
                row.case, row.missing
            );
        }
    }

    /// The refusal names the member the request left out and the type that requires it.
    #[test]
    fn the_refusal_names_the_member_and_the_type() {
        let actual = member_required_for_integration_type(
            SOURCE_APPLICATION_NAME_PARAMETER,
            EVENT_INTEGRATION_TYPE,
        )
        .message;
        assert_eq!(
            actual.as_deref(),
            Some("SourceApplicationName is required for the EVENT integration type"),
            "got {actual:?}"
        );
    }
}
