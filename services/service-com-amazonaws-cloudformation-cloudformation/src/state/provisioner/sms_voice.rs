// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of AWS End User Messaging SMS and Voice's own control
//! plane: the seven `AWS::SMSVOICE::*` resources a template declares. One file of
//! [`super`]'s per-type arms and of its catalogue, dispatched to by its
//! `Provisioner::SmsVoiceResource` variant, which carries which of the seven the
//! declaration is ([`Resource`]).
//!
//! Every one of them is created by that service's own `Create…`/`Put…` call and removed by
//! its own `Delete…`, over the in-process transport an awsJson service is reached on
//! ([`call_json_rpc`] on [`AWS_JSON_1_0`], the protocol its model declares) — so a resource
//! a stack creates is the same resource a client's own call would have created and that
//! service's own `Describe…`/`Get…` reports it.
//!
//! Two kinds of identity live here:
//!
//! * a CONFIGURATION SET and an OPT-OUT LIST are NAMED by the declaration, and that name is
//!   the identifier every call of the owning service addresses one by — so the name the
//!   naming rules resolved is the resource's physical id and the create carries it;
//! * a PROTECT CONFIGURATION, an RCS AGENT, a REGISTRATION and a REGISTRATION ATTACHMENT
//!   are each identified by what the create MINTED (a `ProtectConfigurationId`, an
//!   `RcsAgentId`, a `RegistrationId`, a `RegistrationAttachmentId`), which is why
//!   [`super::super::template::identity_is_service_assigned`] names all four: the values
//!   this emulator's naming rules computed for one are only stand-ins until the create has
//!   answered.
//!
//! A RESOURCE POLICY is neither: it is the document attached to ANOTHER resource, so the
//! ARN it is attached to is its identity — the ARN the declaration states, which is what
//! this type's `Ref` returns and what `PutResourcePolicy`, `GetResourcePolicy` and
//! `DeleteResourcePolicy` each take.
//!
//! A configuration set's own defaults are not members of its create at all: that service
//! sets each through a call of its own (`SetDefaultSenderId`, `SetDefaultMessageType`,
//! `SetDefaultMessageFeedbackEnabled`) and links a protect configuration through
//! `AssociateProtectConfiguration` — so a declaration stating one reaches the set through
//! that call, and a value that service REFUSES (a protect configuration it never minted)
//! fails the resource and rolls the stack back rather than publishing a set the
//! declaration did not ask for.

use super::*;

/// Which of the seven AWS End User Messaging SMS and Voice resources a declaration is,
/// carried by the one `Provisioner` variant these arms are reached through so the seven
/// share one set of arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resource {
    /// `AWS::SMSVOICE::ConfigurationSet`.
    ConfigurationSet,
    /// `AWS::SMSVOICE::OptOutList`.
    OptOutList,
    /// `AWS::SMSVOICE::ProtectConfiguration`.
    ProtectConfiguration,
    /// `AWS::SMSVOICE::ResourcePolicy`.
    Policy,
    /// `AWS::SMSVOICE::RcsAgent`.
    RcsAgent,
    /// `AWS::SMSVOICE::Registration`.
    Registration,
    /// `AWS::SMSVOICE::RegistrationAttachment`.
    RegistrationAttachment,
}

/// The resource-type names of the seven, which the registry of recognised types, the
/// return-value catalogue and the primary-identifier table each key an entry by
/// ([`super::super::template`] re-exports them), re-exported here so an arm names the type
/// it serves by the same const.
pub(in crate::state) use super::super::template::{
    SMSVOICE_CONFIGURATION_SET, SMSVOICE_OPT_OUT_LIST, SMSVOICE_PROTECT_CONFIGURATION,
    SMSVOICE_RCS_AGENT, SMSVOICE_REGISTRATION, SMSVOICE_REGISTRATION_ATTACHMENT,
    SMSVOICE_RESOURCE_POLICY,
};

/// That service's signing name, from its model (`sigv4Name: sms-voice`), and the service
/// shape its awsJson `X-Amz-Target` names the operation under (`PinpointSMSVoiceV2`).
pub(super) const SMS_VOICE_SIGNING_NAME: &str = "sms-voice";
const SMS_VOICE_TARGET_PREFIX: &str = "PinpointSMSVoiceV2";

/// The operations these arms call, as that service's own model names them.
const CREATE_CONFIGURATION_SET: &str = "CreateConfigurationSet";
const DELETE_CONFIGURATION_SET: &str = "DeleteConfigurationSet";
const DESCRIBE_CONFIGURATION_SETS: &str = "DescribeConfigurationSets";
const SET_DEFAULT_SENDER_ID: &str = "SetDefaultSenderId";
const SET_DEFAULT_MESSAGE_TYPE: &str = "SetDefaultMessageType";
const SET_DEFAULT_MESSAGE_FEEDBACK_ENABLED: &str = "SetDefaultMessageFeedbackEnabled";
const ASSOCIATE_PROTECT_CONFIGURATION: &str = "AssociateProtectConfiguration";
const CREATE_OPT_OUT_LIST: &str = "CreateOptOutList";
const DELETE_OPT_OUT_LIST: &str = "DeleteOptOutList";
const DESCRIBE_OPT_OUT_LISTS: &str = "DescribeOptOutLists";
const CREATE_PROTECT_CONFIGURATION: &str = "CreateProtectConfiguration";
const DELETE_PROTECT_CONFIGURATION: &str = "DeleteProtectConfiguration";
const DESCRIBE_PROTECT_CONFIGURATIONS: &str = "DescribeProtectConfigurations";
const PUT_RESOURCE_POLICY: &str = "PutResourcePolicy";
const GET_RESOURCE_POLICY: &str = "GetResourcePolicy";
const DELETE_RESOURCE_POLICY: &str = "DeleteResourcePolicy";
const CREATE_RCS_AGENT: &str = "CreateRcsAgent";
const DELETE_RCS_AGENT: &str = "DeleteRcsAgent";
const DESCRIBE_RCS_AGENTS: &str = "DescribeRcsAgents";
const CREATE_REGISTRATION: &str = "CreateRegistration";
const DELETE_REGISTRATION: &str = "DeleteRegistration";
const DESCRIBE_REGISTRATIONS: &str = "DescribeRegistrations";
const CREATE_REGISTRATION_ATTACHMENT: &str = "CreateRegistrationAttachment";
const DELETE_REGISTRATION_ATTACHMENT: &str = "DeleteRegistrationAttachment";
const DESCRIBE_REGISTRATION_ATTACHMENTS: &str = "DescribeRegistrationAttachments";

/// The error code that service answers for a resource it does not hold
/// (`ResourceNotFoundException`, the modelled 400 of every one of its deletes and reads).
const SMS_VOICE_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `TagList` member every create of that service takes its tags as — the usual
/// `[{Key, Value}]` list ([`UPPER_CASED_TAG`]).
const TAGS_MEMBER: &str = "Tags";

/// The members each create takes the resource's identity in, and the members the creates
/// answer the minted identity and the ARN in. A property of one of these types IS the
/// member of the same name, so there is no re-casing anywhere in this file.
const CONFIGURATION_SET_NAME_MEMBER: &str = "ConfigurationSetName";
const CONFIGURATION_SET_ARN_MEMBER: &str = "ConfigurationSetArn";
const OPT_OUT_LIST_NAME_MEMBER: &str = "OptOutListName";
const OPT_OUT_LIST_ARN_MEMBER: &str = "OptOutListArn";
const PROTECT_CONFIGURATION_ID_MEMBER: &str = "ProtectConfigurationId";
const PROTECT_CONFIGURATION_ARN_MEMBER: &str = "ProtectConfigurationArn";
const RESOURCE_ARN_MEMBER: &str = "ResourceArn";
const POLICY_MEMBER: &str = "Policy";
const RCS_AGENT_ID_MEMBER: &str = "RcsAgentId";
const RCS_AGENT_ARN_MEMBER: &str = "RcsAgentArn";
const REGISTRATION_ID_MEMBER: &str = "RegistrationId";
const REGISTRATION_ARN_MEMBER: &str = "RegistrationArn";
const REGISTRATION_ATTACHMENT_ID_MEMBER: &str = "RegistrationAttachmentId";
const REGISTRATION_ATTACHMENT_ARN_MEMBER: &str = "RegistrationAttachmentArn";

/// The property a resource policy declares the document in, which is the type's own
/// spelling of `PutResourcePolicy`'s `Policy` member.
const POLICY_DOCUMENT_PROPERTY: &str = "PolicyDocument";

/// The property a configuration set declares the protect configuration it is linked to in,
/// which is not a member of its create at all: `AssociateProtectConfiguration` is the call
/// that makes the link, and that call's refusal of an id the service never minted is what
/// fails the resource.
const PROTECT_CONFIGURATION_ID_PROPERTY: &str = "ProtectConfigurationId";

/// The three defaults a configuration set declares, each set by a call of its own after the
/// set exists, with the member that call takes the value as.
const CONFIGURATION_SET_DEFAULTS: &[(&str, &str, &str)] = &[
    ("DefaultSenderId", SET_DEFAULT_SENDER_ID, "SenderId"),
    (
        "DefaultMessageType",
        SET_DEFAULT_MESSAGE_TYPE,
        "MessageType",
    ),
    (
        "MessageFeedbackEnabled",
        SET_DEFAULT_MESSAGE_FEEDBACK_ENABLED,
        "MessageFeedbackEnabled",
    ),
];

/// Every property of each type this file carries onto the owning API, so a declaration
/// stating one outside the set is REFUSED rather than reported as created without it
/// ([`refuse_unmapped_properties`]).
fn mapped_properties(resource_kind: Resource) -> &'static [&'static str] {
    match resource_kind {
        Resource::ConfigurationSet => &[
            CONFIGURATION_SET_NAME_MEMBER,
            "DefaultSenderId",
            "DefaultMessageType",
            "MessageFeedbackEnabled",
            PROTECT_CONFIGURATION_ID_PROPERTY,
            "Tags",
        ],
        Resource::OptOutList => &[OPT_OUT_LIST_NAME_MEMBER, "Tags"],
        Resource::ProtectConfiguration => &["DeletionProtectionEnabled", "Tags"],
        Resource::Policy => &[RESOURCE_ARN_MEMBER, POLICY_DOCUMENT_PROPERTY],
        Resource::RcsAgent => &[
            "DeletionProtectionEnabled",
            OPT_OUT_LIST_NAME_MEMBER,
            "Tags",
        ],
        Resource::Registration => &["RegistrationType", "Tags"],
        Resource::RegistrationAttachment => &["AttachmentBody", "AttachmentUrl", "Tags"],
    }
}

/// Create the AWS End User Messaging SMS and Voice resource a declaration of one of the
/// seven types asks for, so a client that read the stack's outputs finds it in that
/// service's own reads.
///
/// Everything the declaration states is resolved by that service exactly as it is for a
/// client's own call: a name already in use, an opt-out list it does not hold, a protect
/// configuration it never minted — each is that service's refusal, which fails the resource
/// and rolls the stack back rather than publishing an identifier for a resource nothing
/// holds.
pub(super) async fn create_sms_voice_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_kind: Resource,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, mapped_properties(resource_kind))?;
    let properties = resource
        .properties
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    let name = resource.physical_resource_id.clone();
    let mut request = Map::new();
    match resource_kind {
        Resource::ConfigurationSet => {
            request.insert(
                CONFIGURATION_SET_NAME_MEMBER.to_string(),
                Value::String(name.clone()),
            );
        }
        Resource::OptOutList => {
            request.insert(
                OPT_OUT_LIST_NAME_MEMBER.to_string(),
                Value::String(name.clone()),
            );
        }
        Resource::ProtectConfiguration | Resource::RcsAgent => {
            copy_declared(&properties, &["DeletionProtectionEnabled"], &mut request);
            copy_declared(&properties, &[OPT_OUT_LIST_NAME_MEMBER], &mut request);
        }
        Resource::Policy => {
            request.insert(RESOURCE_ARN_MEMBER.to_string(), Value::String(name.clone()));
            request.insert(POLICY_MEMBER.to_string(), declared_policy(&properties)?);
        }
        Resource::Registration => {
            copy_declared(&properties, &["RegistrationType"], &mut request);
        }
        Resource::RegistrationAttachment => {
            copy_declared(&properties, &["AttachmentUrl"], &mut request);
            // The type declares the document's CONTENTS while the API's `AttachmentBody` is
            // a blob, which the awsJson wire carries base64-encoded — through
            // `aws_smithy_types::base64`, the standard-alphabet codec every blob of this
            // crate is rendered with.
            if let Some(body) = properties.get("AttachmentBody").and_then(Value::as_str) {
                request.insert(
                    "AttachmentBody".to_string(),
                    Value::String(::aws_smithy_types::base64::encode(body.as_bytes())),
                );
            }
        }
    }
    // Every create of that service but the policy put takes a `TagList` of its own, so a
    // resource a stack creates carries the tags CloudFormation gives it beside the ones its
    // declaration asks for. A resource policy is a document on another resource and takes
    // no tags at all.
    if resource_kind != Resource::Policy {
        let tags = tags_of(&resource.properties, propagated);
        if !tags.is_empty() {
            request.insert(
                TAGS_MEMBER.to_string(),
                tags_as_list(&tags, UPPER_CASED_TAG),
            );
        }
    }

    let answered = call_sms_voice(
        ctx,
        region,
        create_operation(resource_kind),
        &Value::Object(request),
    )
    .await?;

    // The identity in force: the name the declaration resolved to, for the two NAMED types
    // and for the policy, and what that service MINTED for the other four.
    let physical_resource_id = match identity_member(resource_kind) {
        Some(member) => reported_text(&answered, member)
            .filter(|minted| !minted.is_empty())
            .ok_or_else(|| Refusal {
                // There would be no handle a client could call that service with, and
                // nothing for the stack to publish, so this is a failed create rather than
                // a created resource.
                code: "InternalFailure".to_string(),
                message: format!(
                    "the AWS End User Messaging SMS and Voice control plane answered a \
                     created resource without a {member}"
                ),
            })?,
        None => name,
    };
    let mut attributes = reported_attributes(&answered, published_attributes(resource_kind));

    // A configuration set's declared defaults and its protect-configuration link are each a
    // call of their own, made once the set exists. A declaration the owning service REFUSES
    // there leaves no half-made resource behind: the set this call just created is removed
    // again before the refusal is reported, so the deployment fails with nothing created and
    // a retry of the same template is not refused for a name already in use.
    if resource_kind == Resource::ConfigurationSet {
        if let Err(refusal) =
            apply_configuration_set_declaration(ctx, region, &physical_resource_id, &properties)
                .await
        {
            let _ =
                delete_sms_voice_resource(ctx, region, resource_kind, &physical_resource_id).await;
            return Err(refusal);
        }
    }
    // Every type but the resource policy publishes the ARN that service minted for the
    // resource. A policy publishes no `Fn::GetAtt` attribute at all: its one return value
    // is the ARN its `Ref` returns, which is this resource's physical id.
    if resource_kind != Resource::Policy {
        if let Some(arn) = reported_text(&answered, arn_member(resource_kind)) {
            attributes.insert(
                super::super::template::SMSVOICE_ARN_ATTRIBUTE.to_string(),
                arn,
            );
        }
    }
    Ok(Created {
        physical_resource_id: Some(physical_resource_id),
        attributes,
    })
}

/// Apply a changed declaration to the resource the stack created, for the one type of this
/// family whose owning call is documented to create OR replace what it addresses: a
/// resource policy, which `PutResourcePolicy` attaches to the ARN it names whether or not
/// one is attached already.
///
/// Every other type of this family has no arm here on purpose: that service changes a
/// configuration set's defaults, a protect configuration's deletion protection and an RCS
/// agent's channel through calls this emulator serves no provisioning path for yet, so a
/// changed declaration of one is reported as NOT applied
/// ([`super::provisioning::reconcile`]) rather than as a success that dropped it.
pub(super) async fn update_sms_voice_resource_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let properties = declared.as_object().cloned().unwrap_or_else(Map::new);
    let mut request = Map::new();
    request.insert(
        RESOURCE_ARN_MEMBER.to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    request.insert(POLICY_MEMBER.to_string(), declared_policy(&properties)?);
    call_sms_voice(ctx, region, PUT_RESOURCE_POLICY, &Value::Object(request)).await?;
    Ok(Created::attributes(BTreeMap::new()))
}

/// Delete the resource a stack created, addressed by this resource's physical id — the name
/// the declaration resolved to, the identifier that service minted, or the ARN a policy is
/// attached to.
///
/// One that service no longer holds (`ResourceNotFoundException`, judged on the CODE and
/// never on a message that carries the request's own values) is already in the state the
/// delete asks for.
pub(super) async fn delete_sms_voice_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_kind: Resource,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let mut request = Map::new();
    request.insert(
        address_member(resource_kind).to_string(),
        Value::String(physical_resource_id.to_string()),
    );
    match call_sms_voice(
        ctx,
        region,
        delete_operation(resource_kind),
        &Value::Object(request),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == SMS_VOICE_RESOURCE_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The resource's configuration as that service reports it, for drift detection: its own
/// read of the resource this physical id names. One it no longer holds
/// (`ResourceNotFoundException`, judged on the CODE) was deleted, which is what `None`
/// reports.
///
/// Each of the six reads is a LIST narrowed to the one identifier, so the record is the
/// single entry it answers; a resource policy is read directly and reports only the ARN it
/// is attached to, the document being a declaration-shaped value rather than one that read
/// answers in the type's own spelling — and a property the actual document does not carry is
/// not evidence of anything ([`super::drift`]).
pub(super) async fn describe_sms_voice_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_kind: Resource,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let mut request = Map::new();
    match selection_member(resource_kind) {
        Some(member) => {
            request.insert(
                member.to_string(),
                Value::Array(vec![Value::String(physical_resource_id.to_string())]),
            );
        }
        None => {
            request.insert(
                RESOURCE_ARN_MEMBER.to_string(),
                Value::String(physical_resource_id.to_string()),
            );
        }
    }
    let answered = match call_sms_voice(
        ctx,
        region,
        read_operation(resource_kind),
        &Value::Object(request),
    )
    .await
    {
        Ok(document) => document,
        Err(refusal) if refusal.code == SMS_VOICE_RESOURCE_NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let Some(reported) = reported_entry(resource_kind, &answered) else {
        return Ok(None);
    };
    Ok(Some(reported_as_properties(
        &reported,
        reported_members(resource_kind),
    )))
}

/// Which of the seven a resource type is, or `None` for a type that is none of them — which
/// is what leaves every other type to the answer [`super::provisioner_for`] already gave it.
pub(super) fn sms_voice_provisioner_for(resource_type: &str) -> Option<Resource> {
    match resource_type {
        SMSVOICE_CONFIGURATION_SET => Some(Resource::ConfigurationSet),
        SMSVOICE_OPT_OUT_LIST => Some(Resource::OptOutList),
        SMSVOICE_PROTECT_CONFIGURATION => Some(Resource::ProtectConfiguration),
        SMSVOICE_RESOURCE_POLICY => Some(Resource::Policy),
        SMSVOICE_RCS_AGENT => Some(Resource::RcsAgent),
        SMSVOICE_REGISTRATION => Some(Resource::Registration),
        SMSVOICE_REGISTRATION_ATTACHMENT => Some(Resource::RegistrationAttachment),
        _ => None,
    }
}

/// Call one operation of that service on the awsJson1_0 wire its model declares, naming the
/// operation so a refusal is reported the way that service's own SDK raises it.
async fn call_sms_voice(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        SMS_VOICE_SIGNING_NAME,
        region,
        AWS_JSON_1_0,
        &format!("{SMS_VOICE_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

/// Bring the configuration set the stack just created to the declaration it was created
/// from: each declared default through the call of that service that sets it, and the
/// declared protect configuration through `AssociateProtectConfiguration`.
///
/// The LINK is made last, because it is the one of the four that names another resource: a
/// protect configuration that service never minted is refused there, with that service's own
/// not-found sentence, which is what fails the resource.
async fn apply_configuration_set_declaration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    configuration_set_name: &str,
    properties: &Map<String, Value>,
) -> Result<(), Refusal> {
    for (property, operation, member) in CONFIGURATION_SET_DEFAULTS {
        let Some(declared) = properties.get(*property) else {
            continue;
        };
        let mut request = Map::new();
        request.insert(
            CONFIGURATION_SET_NAME_MEMBER.to_string(),
            Value::String(configuration_set_name.to_string()),
        );
        request.insert((*member).to_string(), declared.clone());
        call_sms_voice(ctx, region, operation, &Value::Object(request)).await?;
    }
    if let Some(declared) = properties.get(PROTECT_CONFIGURATION_ID_PROPERTY) {
        let mut request = Map::new();
        request.insert(
            CONFIGURATION_SET_NAME_MEMBER.to_string(),
            Value::String(configuration_set_name.to_string()),
        );
        request.insert(PROTECT_CONFIGURATION_ID_MEMBER.to_string(), declared.clone());
        call_sms_voice(
            ctx,
            region,
            ASSOCIATE_PROTECT_CONFIGURATION,
            &Value::Object(request),
        )
        .await?;
    }
    Ok(())
}

/// The policy document as `PutResourcePolicy`'s `Policy` member takes it: the TEXT of the
/// document.
///
/// The type declares it either as that text or as the JSON object it is, which is how a
/// template writes a policy inline, so an object is rendered to the text that call's own
/// member carries; a declaration stating none is refused by that operation's own validation
/// rather than silently dropped here.
fn declared_policy(properties: &Map<String, Value>) -> Result<Value, Refusal> {
    let Some(declared) = properties.get(POLICY_DOCUMENT_PROPERTY) else {
        return Ok(Value::String(String::new()));
    };
    Ok(match declared {
        Value::String(text) => Value::String(text.clone()),
        document => Value::String(document.to_string()),
    })
}

/// Copy each declared property the create takes under the SAME member name; a property the
/// declaration does not state sends no member.
fn copy_declared(
    properties: &Map<String, Value>,
    members: &[&str],
    request: &mut Map<String, Value>,
) {
    for member in members {
        if let Some(declared) = properties.get(*member) {
            request.insert((*member).to_string(), declared.clone());
        }
    }
}

/// The operation that creates one of the seven, as that service's own model names it.
fn create_operation(resource_kind: Resource) -> &'static str {
    match resource_kind {
        Resource::ConfigurationSet => CREATE_CONFIGURATION_SET,
        Resource::OptOutList => CREATE_OPT_OUT_LIST,
        Resource::ProtectConfiguration => CREATE_PROTECT_CONFIGURATION,
        Resource::Policy => PUT_RESOURCE_POLICY,
        Resource::RcsAgent => CREATE_RCS_AGENT,
        Resource::Registration => CREATE_REGISTRATION,
        Resource::RegistrationAttachment => CREATE_REGISTRATION_ATTACHMENT,
    }
}

/// The operation that deletes one of the seven.
fn delete_operation(resource_kind: Resource) -> &'static str {
    match resource_kind {
        Resource::ConfigurationSet => DELETE_CONFIGURATION_SET,
        Resource::OptOutList => DELETE_OPT_OUT_LIST,
        Resource::ProtectConfiguration => DELETE_PROTECT_CONFIGURATION,
        Resource::Policy => DELETE_RESOURCE_POLICY,
        Resource::RcsAgent => DELETE_RCS_AGENT,
        Resource::Registration => DELETE_REGISTRATION,
        Resource::RegistrationAttachment => DELETE_REGISTRATION_ATTACHMENT,
    }
}

/// The operation that reads one of the seven, for the drift read above.
fn read_operation(resource_kind: Resource) -> &'static str {
    match resource_kind {
        Resource::ConfigurationSet => DESCRIBE_CONFIGURATION_SETS,
        Resource::OptOutList => DESCRIBE_OPT_OUT_LISTS,
        Resource::ProtectConfiguration => DESCRIBE_PROTECT_CONFIGURATIONS,
        Resource::Policy => GET_RESOURCE_POLICY,
        Resource::RcsAgent => DESCRIBE_RCS_AGENTS,
        Resource::Registration => DESCRIBE_REGISTRATIONS,
        Resource::RegistrationAttachment => DESCRIBE_REGISTRATION_ATTACHMENTS,
    }
}

/// The member a DELETE addresses one of the seven by, which is the member that takes this
/// resource's physical id.
fn address_member(resource_kind: Resource) -> &'static str {
    match resource_kind {
        Resource::ConfigurationSet => CONFIGURATION_SET_NAME_MEMBER,
        Resource::OptOutList => OPT_OUT_LIST_NAME_MEMBER,
        Resource::ProtectConfiguration => PROTECT_CONFIGURATION_ID_MEMBER,
        Resource::Policy => RESOURCE_ARN_MEMBER,
        Resource::RcsAgent => RCS_AGENT_ID_MEMBER,
        Resource::Registration => REGISTRATION_ID_MEMBER,
        Resource::RegistrationAttachment => REGISTRATION_ATTACHMENT_ID_MEMBER,
    }
}

/// The member a create answers the MINTED identity in, for the four types whose identity
/// that service assigns; `None` for a type whose identity is the name or the ARN the
/// declaration already states.
fn identity_member(resource_kind: Resource) -> Option<&'static str> {
    match resource_kind {
        Resource::ConfigurationSet | Resource::OptOutList | Resource::Policy => None,
        Resource::ProtectConfiguration => Some(PROTECT_CONFIGURATION_ID_MEMBER),
        Resource::RcsAgent => Some(RCS_AGENT_ID_MEMBER),
        Resource::Registration => Some(REGISTRATION_ID_MEMBER),
        Resource::RegistrationAttachment => Some(REGISTRATION_ATTACHMENT_ID_MEMBER),
    }
}

/// The member a create answers the resource's ARN in.
fn arn_member(resource_kind: Resource) -> &'static str {
    match resource_kind {
        Resource::ConfigurationSet => CONFIGURATION_SET_ARN_MEMBER,
        Resource::OptOutList => OPT_OUT_LIST_ARN_MEMBER,
        Resource::ProtectConfiguration => PROTECT_CONFIGURATION_ARN_MEMBER,
        Resource::Policy => RESOURCE_ARN_MEMBER,
        Resource::RcsAgent => RCS_AGENT_ARN_MEMBER,
        Resource::Registration => REGISTRATION_ARN_MEMBER,
        Resource::RegistrationAttachment => REGISTRATION_ATTACHMENT_ARN_MEMBER,
    }
}

/// The `(attribute, member)` pairs a type's published return values OTHER than its ARN are
/// read off the create's own answer by — the minted identifier each of the four
/// service-identified types publishes beside it.
fn published_attributes(resource_kind: Resource) -> &'static [(&'static str, &'static str)] {
    match resource_kind {
        Resource::ConfigurationSet | Resource::OptOutList | Resource::Policy => &[],
        Resource::ProtectConfiguration => &[(
            PROTECT_CONFIGURATION_ID_MEMBER,
            PROTECT_CONFIGURATION_ID_MEMBER,
        )],
        Resource::RcsAgent => &[(RCS_AGENT_ID_MEMBER, RCS_AGENT_ID_MEMBER)],
        Resource::Registration => &[(REGISTRATION_ID_MEMBER, REGISTRATION_ID_MEMBER)],
        Resource::RegistrationAttachment => &[(
            REGISTRATION_ATTACHMENT_ID_MEMBER,
            REGISTRATION_ATTACHMENT_ID_MEMBER,
        )],
    }
}

/// The member a READ takes the one identifier to report in, which is a LIST for the six
/// resources that are read out of their own listing; `None` for the resource policy, whose
/// read takes the ARN directly.
fn selection_member(resource_kind: Resource) -> Option<&'static str> {
    match resource_kind {
        Resource::ConfigurationSet => Some("ConfigurationSetNames"),
        Resource::OptOutList => Some("OptOutListNames"),
        Resource::ProtectConfiguration => Some("ProtectConfigurationIds"),
        Resource::Policy => None,
        Resource::RcsAgent => Some("RcsAgentIds"),
        Resource::Registration => Some("RegistrationIds"),
        Resource::RegistrationAttachment => Some("RegistrationAttachmentIds"),
    }
}

/// The one record a read answers about the resource: the first entry of the list the
/// narrowed read reports, or the policy answer itself. A list that reports no entry is a
/// resource that service no longer holds, which is what `None` reports.
fn reported_entry(resource_kind: Resource, answered: &Value) -> Option<Value> {
    let member = match resource_kind {
        Resource::ConfigurationSet => "ConfigurationSets",
        Resource::OptOutList => "OptOutLists",
        Resource::ProtectConfiguration => "ProtectConfigurations",
        Resource::Policy => return Some(answered.clone()),
        Resource::RcsAgent => "RcsAgents",
        Resource::Registration => "Registrations",
        Resource::RegistrationAttachment => "RegistrationAttachments",
    };
    match answered.get(member) {
        Some(Value::Array(entries)) => entries.first().cloned(),
        _ => None,
    }
}

/// The members one of those reads reports under the names the declaring type spells them
/// with, for drift detection.
fn reported_members(resource_kind: Resource) -> &'static [(&'static str, &'static str)] {
    match resource_kind {
        Resource::ConfigurationSet => &[
            (CONFIGURATION_SET_NAME_MEMBER, CONFIGURATION_SET_NAME_MEMBER),
            ("DefaultSenderId", "DefaultSenderId"),
            ("DefaultMessageType", "DefaultMessageType"),
            ("MessageFeedbackEnabled", "DefaultMessageFeedbackEnabled"),
            (
                PROTECT_CONFIGURATION_ID_PROPERTY,
                PROTECT_CONFIGURATION_ID_MEMBER,
            ),
        ],
        Resource::OptOutList => &[(OPT_OUT_LIST_NAME_MEMBER, OPT_OUT_LIST_NAME_MEMBER)],
        Resource::ProtectConfiguration => {
            &[("DeletionProtectionEnabled", "DeletionProtectionEnabled")]
        }
        Resource::Policy => &[(RESOURCE_ARN_MEMBER, RESOURCE_ARN_MEMBER)],
        Resource::RcsAgent => &[
            ("DeletionProtectionEnabled", "DeletionProtectionEnabled"),
            (OPT_OUT_LIST_NAME_MEMBER, OPT_OUT_LIST_NAME_MEMBER),
        ],
        Resource::Registration => &[("RegistrationType", "RegistrationType")],
        Resource::RegistrationAttachment => &[("AttachmentUrl", "AttachmentUrl")],
    }
}
