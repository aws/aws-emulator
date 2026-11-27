// SPDX-License-Identifier: CC0-1.0

//! The TAG SET of one resource of this account: which resource a caller-supplied
//! `resourceArn` names, and where that resource's tags live.
//!
//! A tag is metadata OF A RESOURCE — "A set of key/value pairs that are used to manage
//! the resource" (`smithy_query(query="shape",shape="Tag")`) — so the tags of a resource
//! are stored on the record of that resource, beside everything else it carries, and not
//! in a second store beside the registries. That is what makes a tag set one value rather
//! than two: the tags a create recorded (`CreateThingGroup$tags`,
//! `CreateAuthorizer$tags`, `CreatePackage$tags`, …) and the tags a later write adds are
//! the same field, so a reader of them cannot disagree with either writer.
//!
//! This module is therefore the ONE place that answers two questions for the resource
//! tagging operations (`TagResource`, `UntagResource`, `ListTagsForResource`), each of
//! which addresses its resource by ARN alone:
//!
//! * WHICH resource does this `resourceArn` name ([`parse_tagged_resource`])? The answer
//!   is read from the ARN's resource field, whose `<type>/<id>` form is the one this
//!   service MINTS for that resource type (`super::thing_group_arn`,
//!   `super::authorizer_arn`, …) — so the ARN a create reported is the ARN the tag plane
//!   resolves. A value that is no ARN of this account's IoT namespace is a request to be
//!   FIXED rather than a resource that is missing, so it is refused on the input.
//! * WHERE do that resource's tags live ([`TaggedResource::tag_set_mut`])? One accessor,
//!   which the two writes mutate through and the read walks without writing, so the three
//!   operations cannot drift about where a resource's tags are kept.
//!
//! The ACCOUNT and REGION segments of the ARN are not resolved further than
//! [`super::unaddressable_arn`] reads them, for the reason [`super::parse_job_target`]
//! documents: these registries are keyed by name and by id, and an account's resource is
//! the same resource whichever Region's endpoint minted the ARN a caller kept.

use std::collections::BTreeMap;

use super::ComAmazonawsIotAwsiotserviceState;

/// How many tags ONE resource may carry.
///
/// Modelled: `TagMap` — the shape a create takes a package's tags as — is
/// `length min 1 max 50` (`smithy_query(query="shape",shape="TagMap")`), which is also
/// the limit AWS publishes for every taggable IoT resource and the figure every family's
/// own create already bounds its tag list by ([`super::MAX_POLICY_TAGS`]). A write that
/// would leave a resource holding more is refused with the modelled
/// `LimitExceededException` ("A limit has been exceeded", HTTP 410) rather than recording
/// a resource no create could have made.
pub const MAX_RESOURCE_TAGS: usize = super::MAX_POLICY_TAGS;

/// The resource-type token of a thing group's ARN, as [`super::thing_group_arn`] mints it.
const THING_GROUP_TYPE: &str = "thinggroup";
/// The resource-type token of a billing group's ARN, as [`super::billing_group_arn`] mints
/// it.
const BILLING_GROUP_TYPE: &str = "billinggroup";
/// The resource-type token of a thing type's ARN, as [`super::thing_type_arn`] mints it.
const THING_TYPE_TYPE: &str = "thingtype";
/// The resource-type token of a policy's ARN, as [`super::policy_arn`] mints it.
const POLICY_TYPE: &str = "policy";
/// The resource-type token of an authorizer's ARN, as [`super::authorizer_arn`] mints it.
const AUTHORIZER_TYPE: &str = "authorizer";
/// The resource-type token of a command's ARN, as [`super::command_arn`] mints it.
const COMMAND_TYPE: &str = "command";
/// The resource-type token of a package's ARN, as [`super::package_arn`] mints it. A
/// package VERSION's ARN extends it with [`PACKAGE_VERSION_INFIX`]
/// ([`super::package_version_arn`]).
const PACKAGE_TYPE: &str = "package";
/// What separates a package's name from one of its version names inside a package
/// version's ARN resource field (`package/<packageName>/version/<versionName>`).
const PACKAGE_VERSION_INFIX: &str = "/version/";
/// The resource-type token of a stream's ARN, as [`super::stream_arn`] mints it.
const STREAM_TYPE: &str = "stream";
/// The resource-type token of a fleet metric's ARN, as [`super::fleet_metric_arn`] mints
/// it.
const FLEET_METRIC_TYPE: &str = "fleetmetric";
/// The resource-type token of a role alias's ARN, as [`super::role_alias_arn`] mints it.
const ROLE_ALIAS_TYPE: &str = "rolealias";
/// The resource-type token of a domain configuration's ARN, as
/// [`super::domain_configuration_arn`] mints it.
const DOMAIN_CONFIGURATION_TYPE: &str = "domainconfiguration";
/// The resource-type token of a provisioning template's ARN, as
/// [`super::provisioning_template_arn`] mints it.
const PROVISIONING_TEMPLATE_TYPE: &str = "provisioningtemplate";
/// The resource-type token of a CA certificate's ARN, as [`super::ca_certificate_arn`]
/// mints it.
const CA_CERTIFICATE_TYPE: &str = "cacert";
/// The resource-type token of a certificate provider's ARN, as
/// [`super::certificate_provider_arn`] mints it.
const CERTIFICATE_PROVIDER_TYPE: &str = "certificateprovider";
/// The resource-type token of a topic rule's ARN, as [`super::topic_rule_arn`] mints it.
const TOPIC_RULE_TYPE: &str = "rule";
/// The resource-type token of a scheduled audit's ARN, as
/// [`super::scheduled_audit_arn`] mints it.
const SCHEDULED_AUDIT_TYPE: &str = "scheduledaudit";

/// Which resource of this account a `resourceArn` names, for the operations that address
/// a resource by ARN alone.
///
/// One variant per resource FAMILY this service keeps a tag set for, each carrying the
/// identifier that family's store is keyed by — the name or id the ARN's resource field
/// spells — so resolving the ARN and reading the store are two steps and the second one
/// needs nothing of the ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// `thinggroup/<thingGroupName>`: a thing group, static or dynamic.
    ThingGroup(String),
    /// `billinggroup/<billingGroupName>`: a billing group.
    BillingGroup(String),
    /// `thingtype/<thingTypeName>`: a thing type.
    ThingType(String),
    /// `policy/<policyName>`: an IoT policy.
    Policy(String),
    /// `authorizer/<authorizerName>`: a custom authorizer.
    Authorizer(String),
    /// `command/<commandId>`: a command.
    Command(String),
    /// `package/<packageName>`: a software package.
    Package(String),
    /// `package/<packageName>/version/<versionName>`: one version of a software package,
    /// whose identity is the PAIR.
    PackageVersion(String, String),
    /// `stream/<streamId>`: a stream.
    Stream(String),
    /// `fleetmetric/<metricName>`: a fleet metric.
    FleetMetric(String),
    /// `rolealias/<roleAlias>`: a role alias.
    RoleAlias(String),
    /// `domainconfiguration/<domainConfigurationName>`: a domain configuration.
    DomainConfiguration(String),
    /// `provisioningtemplate/<templateName>`: a provisioning template.
    ProvisioningTemplate(String),
    /// `cacert/<certificateId>`: a registered CA certificate.
    CaCertificate(String),
    /// `certificateprovider/<certificateProviderName>`: a certificate provider.
    CertificateProvider(String),
    /// `rule/<ruleName>`: a topic rule.
    TopicRule(String),
    /// `scheduledaudit/<scheduledAuditName>`: a Device Defender scheduled audit.
    ScheduledAudit(String),
}

/// Which resource `resource_arn` names, or the sentence a request naming no resource of
/// this account's IoT namespace is refused with.
///
/// The refusal is a BAD REQUEST (`InvalidRequestException`, HTTP 400) and not a
/// not-found: a value that is not an ARN, an ARN of another account or an ARN whose
/// resource field names no resource type this service tags addresses nothing in this
/// namespace whatever the account's state holds, so it has to be fixed rather than
/// retried. A well-formed ARN of a family this service DOES tag is resolved against the
/// store instead ([`TaggedResource::tag_set_mut`]), where a lookup miss is the modelled
/// not-found.
///
/// WHICH sentence depends on what about the ARN cannot be acted on: an ARN of this
/// namespace whose resource TYPE carries no tag set names that type
/// ([`untaggable_resource_type_message`]), because the kind of resource is what the caller
/// has to change, while a value no resource type can be read out of at all is refused as
/// the value it is ([`untaggable_resource_message`]).
///
/// The leading-`arn:` and account readings are [`super::unaddressable_arn`]'s, the one
/// reading of a caller-supplied ARN this service already has, so the tag plane and the
/// topic-rule destination plane refuse the same value in the same words.
pub fn parse_tagged_resource(resource_arn: &str) -> Result<TaggedResource, String> {
    if let Some(message) = super::unaddressable_arn(resource_arn) {
        return Err(message);
    }
    let resource = ::service_common::arn::parse_of(resource_arn, IOT_VENDOR)
        .map(|arn| arn.resource)
        .ok_or_else(|| untaggable_resource_message(resource_arn))?;
    let (resource_type, identifier) = resource
        .split_once('/')
        .filter(|(_, identifier)| !identifier.is_empty())
        .ok_or_else(|| untaggable_resource_message(resource_arn))?;
    // The resource TYPE is what decides this, so a type no family of this plane holds a tag
    // set for is refused by NAMING that type ([`untaggable_resource_type_message`]) —
    // whichever resource of it the ARN went on to name.
    let untaggable = || untaggable_resource_type_message(resource_type);
    match resource_type {
        THING_GROUP_TYPE => Ok(TaggedResource::ThingGroup(identifier.to_string())),
        BILLING_GROUP_TYPE => Ok(TaggedResource::BillingGroup(identifier.to_string())),
        THING_TYPE_TYPE => Ok(TaggedResource::ThingType(identifier.to_string())),
        POLICY_TYPE => Ok(TaggedResource::Policy(identifier.to_string())),
        AUTHORIZER_TYPE => Ok(TaggedResource::Authorizer(identifier.to_string())),
        COMMAND_TYPE => Ok(TaggedResource::Command(identifier.to_string())),
        // A package version's ARN is its package's ARN with a further segment, so the
        // two families are told apart by that segment and not by the type token.
        PACKAGE_TYPE => match identifier.split_once(PACKAGE_VERSION_INFIX) {
            Some((package_name, version_name))
                if !package_name.is_empty() && !version_name.is_empty() =>
            {
                Ok(TaggedResource::PackageVersion(
                    package_name.to_string(),
                    version_name.to_string(),
                ))
            }
            Some(_) => Err(untaggable_resource_message(resource_arn)),
            None => Ok(TaggedResource::Package(identifier.to_string())),
        },
        STREAM_TYPE => Ok(TaggedResource::Stream(identifier.to_string())),
        FLEET_METRIC_TYPE => Ok(TaggedResource::FleetMetric(identifier.to_string())),
        ROLE_ALIAS_TYPE => Ok(TaggedResource::RoleAlias(identifier.to_string())),
        DOMAIN_CONFIGURATION_TYPE => {
            Ok(TaggedResource::DomainConfiguration(identifier.to_string()))
        }
        PROVISIONING_TEMPLATE_TYPE => {
            Ok(TaggedResource::ProvisioningTemplate(identifier.to_string()))
        }
        CA_CERTIFICATE_TYPE => Ok(TaggedResource::CaCertificate(identifier.to_string())),
        CERTIFICATE_PROVIDER_TYPE => {
            Ok(TaggedResource::CertificateProvider(identifier.to_string()))
        }
        TOPIC_RULE_TYPE => Ok(TaggedResource::TopicRule(identifier.to_string())),
        SCHEDULED_AUDIT_TYPE => Ok(TaggedResource::ScheduledAudit(identifier.to_string())),
        _ => Err(untaggable()),
    }
}

/// This service's ARN namespace, which an ARN the tag plane acts on must be of
/// (`smithy_query(query="service_info")`: `awsService.arnNamespace`).
const IOT_VENDOR: &str = "iot";

/// The sentence a `resourceArn` whose RESOURCE TYPE is no family this service tags is
/// refused with.
///
/// The refusal is about the TYPE and not about the particular resource: an IoT thing ARN,
/// say, is a well-formed ARN of this account naming a resource that carries no tag set at
/// all, so what a caller has to change is the KIND of resource it addressed. The fixed text
/// is therefore `Invalid resource type in ARN: ` and the one variable part is the resource
/// type read out of the request's own `resourceArn` — never the whole ARN, which tells a
/// caller nothing about which part of it this plane cannot act on.
fn untaggable_resource_type_message(resource_type: &str) -> String {
    format!("Invalid resource type in ARN: {resource_type}")
}

/// The sentence a `resourceArn` that names no resource this service tags is refused with.
///
/// AWS publishes the CLASS for this — `InvalidRequestException`, "The request is not
/// valid" — and not the sentence it states it in, so the fixed text here is this
/// emulator's rendering of that class: it names the MEMBER the value was bound at, the
/// way this service already states a member whose value it could not read
/// ([`super::invalid_cognito_identity_pool_id_message`]), and the only variable part is
/// the value the request sent.
fn untaggable_resource_message(resource_arn: &str) -> String {
    format!("Invalid ResourceArn: {resource_arn}")
}

/// The sentence a tag write whose `@required` collection arrived EMPTY is refused with.
///
/// `TagResource$tags` and `UntagResource$tagKeys` are both `@required`
/// (`smithy_query(query="operation",operation="TagResource,UntagResource")`), so each
/// write STATES the collection it acts with; a collection that is there and holds nothing
/// states no tag to write and no key to remove, and AWS refuses it on the input rather
/// than answering a success that changed nothing. The class is the modelled
/// `InvalidRequestException` (HTTP 400) and the sentence is entirely fixed text — it names
/// neither the member nor the resource — so it is a constant rather than a template.
///
/// The recorded refusal is `UntagResource`'s; the `TagResource` half is the same rule read
/// on the other write's collection, which is the one both of them are validated by.
pub const EMPTY_TAG_COLLECTION_MESSAGE: &str = "The validated collection is empty";

impl TaggedResource {
    /// The tag set of the resource this names, or `None` when the account holds no such
    /// resource.
    ///
    /// The ONE accessor: `TagResource` and `UntagResource` write through it, and
    /// `ListTagsForResource` reads what it hands back and writes nothing, so a tag the
    /// write stored is read out of the same field it was stored in. A create's tags are
    /// in that same field, which is why the tags a resource was created with are the tags
    /// the read reports.
    pub fn tag_set_mut<'store>(
        &self,
        store: &'store mut ComAmazonawsIotAwsiotserviceState,
    ) -> Option<ResourceTagSet<'store>> {
        match self {
            Self::ThingGroup(name) => store
                .thing_groups
                .get_mut(name)
                .map(|group| ResourceTagSet::Optional(&mut group.tags)),
            Self::BillingGroup(name) => store
                .billing_groups
                .get_mut(name)
                .map(|group| ResourceTagSet::Optional(&mut group.tags)),
            Self::ThingType(name) => store
                .thing_types
                .get_mut(name)
                .map(|thing_type| ResourceTagSet::Optional(&mut thing_type.tags)),
            Self::Policy(name) => store
                .policies
                .get_mut(name)
                .map(|policy| ResourceTagSet::Optional(&mut policy.tags)),
            Self::Authorizer(name) => store
                .authorizers
                .get_mut(name)
                .map(|authorizer| ResourceTagSet::Optional(&mut authorizer.tags)),
            Self::Command(id) => store
                .commands
                .get_mut(id)
                .map(|command| ResourceTagSet::Optional(&mut command.tags)),
            Self::Package(name) => store
                .packages
                .get_mut(name)
                .map(|package| ResourceTagSet::Valued(&mut package.tags)),
            Self::PackageVersion(package_name, version_name) => store
                .packages
                .get_mut(package_name)
                .and_then(|package| package.versions.get_mut(version_name))
                .map(|version| ResourceTagSet::Valued(&mut version.tags)),
            Self::Stream(id) => store
                .streams
                .get_mut(id)
                .map(|stream| ResourceTagSet::Optional(&mut stream.tags)),
            Self::FleetMetric(name) => store
                .fleet_metrics
                .get_mut(name)
                .map(|metric| ResourceTagSet::Optional(&mut metric.tags)),
            Self::RoleAlias(alias) => store
                .role_aliases
                .get_mut(alias)
                .map(|role_alias| ResourceTagSet::Optional(&mut role_alias.tags)),
            Self::DomainConfiguration(name) => store
                .domain_configurations
                .get_mut(name)
                .map(|configuration| ResourceTagSet::Optional(&mut configuration.tags)),
            Self::ProvisioningTemplate(name) => store
                .provisioning_templates
                .get_mut(name)
                .map(|template| ResourceTagSet::Optional(&mut template.tags)),
            Self::CaCertificate(id) => store
                .ca_certificates
                .get_mut(id)
                .map(|certificate| ResourceTagSet::Optional(&mut certificate.tags)),
            Self::CertificateProvider(name) => store
                .certificate_providers
                .get_mut(name)
                .map(|provider| ResourceTagSet::Optional(&mut provider.tags)),
            Self::TopicRule(name) => store
                .topic_rules
                .get_mut(name)
                .map(|rule| ResourceTagSet::Optional(&mut rule.tags)),
            Self::ScheduledAudit(name) => store
                .scheduled_audits
                .get_mut(name)
                .map(|scheduled_audit| ResourceTagSet::Optional(&mut scheduled_audit.tags)),
        }
    }

    /// How the tag plane states that this ARN names no resource the account holds: the
    /// sentence the resource's OWN family refuses a lookup miss with, so a tag call and a
    /// describe of the same missing resource are told the same thing — less the families
    /// the service itself words the two planes differently for, where this plane's half is
    /// the one named here ([`super::not_found`]).
    pub fn missing_message(&self) -> String {
        match self {
            // The THING GROUP family is one AWS words the two planes apart for: a tag call
            // addressing a group by ARN is told this plane's sentence and not the one a
            // reader or writer of the group itself is told.
            Self::ThingGroup(name) => super::tagged_thing_group_not_found_message(name),
            Self::BillingGroup(name) => super::missing_billing_group_message(name),
            Self::ThingType(name) => super::missing_thing_type_message(name),
            Self::Policy(name) => super::missing_policy_message(name),
            Self::Authorizer(name) => super::missing_authorizer_message(name),
            Self::Command(id) => super::missing_command_message(id),
            Self::Package(name) => super::missing_package_message(name),
            Self::PackageVersion(package_name, version_name) => {
                super::missing_package_version_message(package_name, version_name)
            }
            Self::Stream(id) => super::missing_stream_message(id),
            Self::FleetMetric(name) => super::missing_fleet_metric_message(name),
            Self::RoleAlias(alias) => super::missing_role_alias_message(alias),
            Self::DomainConfiguration(name) => super::missing_domain_configuration_message(name),
            Self::ProvisioningTemplate(name) => super::missing_provisioning_template_message(name),
            // A CA certificate is addressed by the same `certificateId` member a device
            // certificate is, and every reader of a certificate this service holds none
            // of states the miss in one sentence (`ops::describe_ca_certificate`).
            Self::CaCertificate(id) => super::missing_certificate_message(id),
            Self::CertificateProvider(name) => super::missing_certificate_provider_message(name),
            Self::TopicRule(name) => missing_topic_rule_message(name),
            Self::ScheduledAudit(name) => super::missing_scheduled_audit_message(name),
        }
    }
}

/// How the tag plane states that a `rule/<ruleName>` ARN names no topic rule this account
/// holds.
///
/// The operations that address one rule by NAME answer a miss with a DENIAL, because AWS
/// authorizes each of them against the rule's own ARN and none of them models a not-found
/// error at all ([`super::unauthorized_topic_rule_message`]). The tagging operations are
/// addressed by the ARN the request itself carries and DO model
/// `ResourceNotFoundException`, so a miss here is stated as one: AWS publishes the class
/// and not the sentence, so the fixed text is this emulator's rendering of it — the
/// family noun and the name the ARN carried, the form the authorizer family's published
/// sentence takes.
fn missing_topic_rule_message(rule_name: &str) -> String {
    format!("Rule {rule_name} not found")
}

/// One resource's tag set, as the record that owns it stores it.
///
/// Two variants because the model takes a resource's tags in two shapes, and each family
/// stores what ITS create was given: `TagList`'s `Tag$Value` is OPTIONAL
/// (`smithy_query(query="shape",shape="Tag")`), while the software-package family takes a
/// `TagMap`, whose values are plain strings. A tag written through this type keeps the
/// distinction its owner's shape makes, so a reader reports what the resource holds
/// rather than a value this type invented.
#[derive(Debug)]
pub enum ResourceTagSet<'store> {
    /// A family whose tags are the `TagList` pairs the model declares: a key, and a value
    /// the writer may have left out.
    Optional(&'store mut BTreeMap<String, Option<String>>),
    /// The software-package family, whose tags are a `TagMap`: every key has a value, so a
    /// pair written with none carries the empty value `TagValue` admits
    /// (`length min 0`).
    Valued(&'store mut BTreeMap<String, String>),
}

impl ResourceTagSet<'_> {
    /// The tags of the resource, in tag-key order — the order the store keeps them in, so
    /// two reads of an unchanged resource report one order.
    pub fn tags(&self) -> Vec<(String, Option<String>)> {
        match self {
            Self::Optional(tags) => tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            Self::Valued(tags) => tags
                .iter()
                .map(|(key, value)| (key.clone(), Some(value.clone())))
                .collect(),
        }
    }

    /// The tag keys of the resource.
    pub fn keys(&self) -> Vec<String> {
        match self {
            Self::Optional(tags) => tags.keys().cloned().collect(),
            Self::Valued(tags) => tags.keys().cloned().collect(),
        }
    }

    /// Add the tag `key` or MODIFY the one already stored under it.
    ///
    /// `Tag$Key` is `@required` and keys the set, so a key already present is replaced
    /// rather than duplicated: `TagResource` "Adds to or modifies the tags of the given
    /// resource", which is one operation over one keyed set.
    pub fn put(&mut self, key: String, value: Option<String>) {
        match self {
            Self::Optional(tags) => {
                tags.insert(key, value);
            }
            Self::Valued(tags) => {
                tags.insert(key, value.unwrap_or_default());
            }
        }
    }

    /// Remove the tag `key` carries, if the resource carries one.
    pub fn remove(&mut self, key: &str) {
        match self {
            Self::Optional(tags) => {
                tags.remove(key);
            }
            Self::Valued(tags) => {
                tags.remove(key);
            }
        }
    }
}

/// The tag set a `key1=value1&key2=value2` query string carries.
///
/// `CreateTopicRule$tags` is the one place this service takes a resource's tags as that
/// raw string — "For URI Request parameters use format: ...key1=value1&key2=value2"
/// (`smithy_query(query="doc",shape="CreateTopicRuleRequest",member="tags")`) — so it is
/// read into the same keyed set every other family stores, and the rule's tags are then
/// one value the tagging operations and the create share.
///
/// Read with `form_urlencoded`, the one reader of a query string this crate has, so a tag
/// whose key or value carries a character of the form's own grammar arrives
/// percent-decoded rather than split through. Every key the form carries gets a VALUE:
/// the form has no way to say that `Tag$Value` was left out, and a pair written without
/// one names the empty value `TagValue` admits (`length min 0`).
pub fn tags_of_tag_query(tags: Option<&str>) -> BTreeMap<String, Option<String>> {
    ::form_urlencoded::parse(tags.unwrap_or_default().as_bytes())
        .map(|(key, value)| (key.into_owned(), Some(value.into_owned())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ARN this service mints for a resource is the ARN the tag plane resolves back
    /// to that resource: every family's own builder, read by the one parse.
    #[test]
    fn every_minted_arn_resolves_to_the_resource_it_names() {
        let region = "eu-west-2";
        let cases: Vec<(String, TaggedResource)> = vec![
            (
                crate::state::thing_group_arn(region, "fleet"),
                TaggedResource::ThingGroup("fleet".to_string()),
            ),
            (
                crate::state::billing_group_arn(region, "metered"),
                TaggedResource::BillingGroup("metered".to_string()),
            ),
            (
                crate::state::thing_type_arn(region, "sensor"),
                TaggedResource::ThingType("sensor".to_string()),
            ),
            (
                crate::state::policy_arn(region, "publish"),
                TaggedResource::Policy("publish".to_string()),
            ),
            (
                crate::state::authorizer_arn(region, "gatekeeper"),
                TaggedResource::Authorizer("gatekeeper".to_string()),
            ),
            (
                crate::state::command_arn(region, "reboot"),
                TaggedResource::Command("reboot".to_string()),
            ),
            (
                crate::state::package_arn(region, "firmware"),
                TaggedResource::Package("firmware".to_string()),
            ),
            (
                crate::state::package_version_arn(region, "firmware", "1.0.0"),
                TaggedResource::PackageVersion("firmware".to_string(), "1.0.0".to_string()),
            ),
            (
                crate::state::stream_arn(region, "chunks"),
                TaggedResource::Stream("chunks".to_string()),
            ),
            (
                crate::state::fleet_metric_arn(region, "connected"),
                TaggedResource::FleetMetric("connected".to_string()),
            ),
            (
                crate::state::role_alias_arn(region, "credentials"),
                TaggedResource::RoleAlias("credentials".to_string()),
            ),
            (
                crate::state::domain_configuration_arn(region, "iot-domain"),
                TaggedResource::DomainConfiguration("iot-domain".to_string()),
            ),
            (
                crate::state::provisioning_template_arn(region, "fleet-template"),
                TaggedResource::ProvisioningTemplate("fleet-template".to_string()),
            ),
            (
                crate::state::ca_certificate_arn(region, "abc123"),
                TaggedResource::CaCertificate("abc123".to_string()),
            ),
            (
                crate::state::certificate_provider_arn(region, "issuer"),
                TaggedResource::CertificateProvider("issuer".to_string()),
            ),
            (
                crate::state::topic_rule_arn(region, "temperature"),
                TaggedResource::TopicRule("temperature".to_string()),
            ),
            (
                crate::state::scheduled_audit_arn(region, "nightly"),
                TaggedResource::ScheduledAudit("nightly".to_string()),
            ),
        ];
        for (arn, expected) in cases {
            let actual = parse_tagged_resource(&arn);
            assert_eq!(
                actual.as_ref(),
                Ok(&expected),
                "case {arn:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A value that addresses no resource of this account's IoT namespace is refused on
    /// the input, whatever the account holds.
    #[test]
    fn a_value_addressing_no_iot_resource_is_refused() {
        let account = ::engine::emulator_account_id();
        let cases = [
            "fleet",
            "arn:aws:s3:::bucket",
            "arn:aws:iot:us-east-1:000000000001:thinggroup/fleet",
            "arn:aws:iot:us-east-1:000000000000:thinggroup",
            "arn:aws:iot:us-east-1:000000000000:thinggroup/",
            "arn:aws:iot:us-east-1:000000000000:subscription/42",
        ];
        for case in cases {
            let case = case.replace("000000000000", account);
            let actual = parse_tagged_resource(&case);
            assert!(
                actual.is_err(),
                "case {case:?}: expected a refusal, got {actual:?}"
            );
        }
    }

    /// One row of the query-string table: the raw `tags` value a create carried, and the
    /// tag pairs it has to read as.
    type QueryCase<'row> = (Option<&'row str>, Vec<(&'row str, Option<&'row str>)>);

    /// The raw tag query string a topic rule's create carries is read as the keyed set
    /// every other family stores.
    #[test]
    fn a_tag_query_string_reads_as_a_keyed_tag_set() {
        let cases: Vec<QueryCase<'_>> = vec![
            (None, vec![]),
            (Some(""), vec![]),
            (Some("stage=prod"), vec![("stage", Some("prod"))]),
            (
                Some("stage=prod&owner=fleet-team"),
                vec![("owner", Some("fleet-team")), ("stage", Some("prod"))],
            ),
            (Some("probe"), vec![("probe", Some(""))]),
            (Some("stage="), vec![("stage", Some(""))]),
            (
                Some("stage=prod&stage=staging"),
                vec![("stage", Some("staging"))],
            ),
            // The form's own separators arrive percent-decoded, which is what keeps a tag
            // key carrying one from being split through.
            (
                Some("cost%3Dcenter=fleet%26ops"),
                vec![("cost=center", Some("fleet&ops"))],
            ),
        ];
        for (query, expected) in cases {
            let actual = tags_of_tag_query(query);
            let expected: BTreeMap<String, Option<String>> = expected
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.map(str::to_string)))
                .collect();
            assert_eq!(
                actual, expected,
                "case {query:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
