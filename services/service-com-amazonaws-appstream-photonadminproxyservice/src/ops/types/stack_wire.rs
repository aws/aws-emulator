// SPDX-License-Identifier: CC0-1.0

//! The `Stack` every reader of a stack answers with, and the one member of it the on-disk
//! model does not declare.
//!
//! WHY THIS MODULE EXISTS. `#[smithy_macros::smithy(shape = ...)]` derives a structure's
//! `Schema`, its serializer and its deserializer from the on-disk model's member list, and
//! it admits EXACTLY that list: a Rust field with no member behind it is a compile error,
//! so a member the service really reports but the model omits cannot be declared through
//! the derive at all. A stack reports its lifecycle `State` beside the configuration it
//! holds — the member a client gates on before streaming from the stack — and the shape's
//! modelled member list does not mention it, so this module emits for `Stack` the same
//! items the derive emits: one member schema per member (index = its position in the
//! member list), the struct `SCHEMA` naming them in that order, the `STACK_SCHEMA` static
//! and the `SerializableStruct`/`DeserializableStruct` pair.
//!
//! The model's own members keep the indices and the order the model gives them (the order
//! `smithy_query(query="shape", shape="Stack")` lists them in); the member it does not
//! declare is appended LAST, with its shape id in a namespace of its own
//! ([`RECORDED_NAMESPACE`]), so it can neither claim to be modelled nor collide with a
//! member the model may declare later — a model refresh only has to move it into place and
//! put the derive back. The ids never leave the process: the codec writes a member's NAME
//! as the key.
//!
//! WHAT IS DELIBERATELY ABSENT, for the same reason the derive's own output would be inert
//! here: the constraint traits (the validating walk runs on operation INPUTS only, and this
//! structure is output-only), the `@http` chain (baked on only for an operation's input)
//! and `@jsonName` (no member here declares one: each one's wire name IS its member name).
//!
//! Nothing here decides a VALUE: every member is written from what the stored stack holds
//! (`crate::state::StackRecord::to_stack`).

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};
use ::aws_smithy_schema_server::DeserializableStruct;
use ::aws_smithy_types::DateTime;

use super::{
    AccessEndpoint, AgentAccessConfig, ApplicationSettingsResponse, ContentRedirection, StackError,
    StorageConnector, StreamingExperienceSettings, UserSetting, WebAuthnRedirection,
};

/// The namespace the member the on-disk model does not declare sits in: NOT this service's
/// own, so a hand-written member can never be mistaken for a modelled one.
const RECORDED_NAMESPACE: &str = "com.amazonaws.appstream.recorded";

/// Describes a stack, as every operation that reports one answers it.
///
/// The first eighteen members are `com.amazonaws.appstream#Stack`'s, in the model's member
/// order. `state` is the one AppStream reports beyond the published model: the stack's own
/// lifecycle state.
#[derive(Debug, Clone, Default)]
pub struct Stack {
    pub arn: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub created_time: Option<DateTime>,
    pub storage_connectors: Option<Vec<StorageConnector>>,
    pub redirect_url: Option<String>,
    pub feedback_url: Option<String>,
    pub stack_errors: Option<Vec<StackError>>,
    pub user_settings: Option<Vec<UserSetting>>,
    pub application_settings: Option<ApplicationSettingsResponse>,
    pub access_endpoints: Option<Vec<AccessEndpoint>>,
    pub embed_host_domains: Option<Vec<String>>,
    pub streaming_experience_settings: Option<StreamingExperienceSettings>,
    pub content_redirection: Option<ContentRedirection>,
    pub web_authn_redirection: Option<WebAuthnRedirection>,
    pub agent_access_config: Option<AgentAccessConfig>,
    pub storage_connector_authentication: Option<String>,
    /// The stack's lifecycle state, the member the on-disk model does not declare.
    pub state: Option<String>,
}

/// One member this service's model DOES declare: its shape id in the service's own
/// namespace, its shape type, its wire name and its index in the member list — the member
/// schema the derive would emit for it.
macro_rules! modelled_member {
    ($wire:literal, $shape_type:expr, $index:literal) => {
        Schema::new_member(
            ShapeId::from_static(
                concat!("com.amazonaws.appstream#Stack$", $wire),
                "com.amazonaws.appstream",
                $wire,
            ),
            $shape_type,
            $wire,
            $index,
        )
    };
}

impl Stack {
    pub const M_ARN: Schema = modelled_member!("Arn", ShapeType::String, 0);
    pub const M_NAME: Schema = modelled_member!("Name", ShapeType::String, 1);
    pub const M_DESCRIPTION: Schema = modelled_member!("Description", ShapeType::String, 2);
    pub const M_DISPLAY_NAME: Schema = modelled_member!("DisplayName", ShapeType::String, 3);
    pub const M_CREATED_TIME: Schema = modelled_member!("CreatedTime", ShapeType::Timestamp, 4);
    pub const M_STORAGE_CONNECTORS: Schema =
        modelled_member!("StorageConnectors", ShapeType::List, 5);
    pub const M_REDIRECT_URL: Schema = modelled_member!("RedirectURL", ShapeType::String, 6);
    pub const M_FEEDBACK_URL: Schema = modelled_member!("FeedbackURL", ShapeType::String, 7);
    pub const M_STACK_ERRORS: Schema = modelled_member!("StackErrors", ShapeType::List, 8);
    pub const M_USER_SETTINGS: Schema = modelled_member!("UserSettings", ShapeType::List, 9);
    pub const M_APPLICATION_SETTINGS: Schema =
        modelled_member!("ApplicationSettings", ShapeType::Structure, 10);
    pub const M_ACCESS_ENDPOINTS: Schema =
        modelled_member!("AccessEndpoints", ShapeType::List, 11);
    pub const M_EMBED_HOST_DOMAINS: Schema =
        modelled_member!("EmbedHostDomains", ShapeType::List, 12);
    pub const M_STREAMING_EXPERIENCE_SETTINGS: Schema =
        modelled_member!("StreamingExperienceSettings", ShapeType::Structure, 13);
    pub const M_CONTENT_REDIRECTION: Schema =
        modelled_member!("ContentRedirection", ShapeType::Structure, 14);
    pub const M_WEB_AUTHN_REDIRECTION: Schema =
        modelled_member!("WebAuthnRedirection", ShapeType::Structure, 15);
    pub const M_AGENT_ACCESS_CONFIG: Schema =
        modelled_member!("AgentAccessConfig", ShapeType::Structure, 16);
    pub const M_STORAGE_CONNECTOR_AUTHENTICATION: Schema =
        modelled_member!("StorageConnectorAuthentication", ShapeType::String, 17);

    /// The one member the model does not declare, appended last and in
    /// [`RECORDED_NAMESPACE`].
    pub const M_STATE: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.appstream.recorded#Stack$State",
            RECORDED_NAMESPACE,
            "State",
        ),
        ShapeType::String,
        "State",
        18,
    );

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.appstream#Stack",
            "com.amazonaws.appstream",
            "Stack",
        ),
        ShapeType::Structure,
        &[
            &Self::M_ARN,
            &Self::M_NAME,
            &Self::M_DESCRIPTION,
            &Self::M_DISPLAY_NAME,
            &Self::M_CREATED_TIME,
            &Self::M_STORAGE_CONNECTORS,
            &Self::M_REDIRECT_URL,
            &Self::M_FEEDBACK_URL,
            &Self::M_STACK_ERRORS,
            &Self::M_USER_SETTINGS,
            &Self::M_APPLICATION_SETTINGS,
            &Self::M_ACCESS_ENDPOINTS,
            &Self::M_EMBED_HOST_DOMAINS,
            &Self::M_STREAMING_EXPERIENCE_SETTINGS,
            &Self::M_CONTENT_REDIRECTION,
            &Self::M_WEB_AUTHN_REDIRECTION,
            &Self::M_AGENT_ACCESS_CONFIG,
            &Self::M_STORAGE_CONNECTOR_AUTHENTICATION,
            &Self::M_STATE,
        ],
    );
}

pub static STACK_SCHEMA: &Schema = &Stack::SCHEMA;

impl SerializableStruct for Stack {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        if let Some(value) = self.arn.as_ref() {
            s.write_string(&Self::M_ARN, value.as_str())?;
        }
        s.write_string(&Self::M_NAME, self.name.as_str())?;
        if let Some(value) = self.description.as_ref() {
            s.write_string(&Self::M_DESCRIPTION, value.as_str())?;
        }
        if let Some(value) = self.display_name.as_ref() {
            s.write_string(&Self::M_DISPLAY_NAME, value.as_str())?;
        }
        if let Some(value) = self.created_time.as_ref() {
            s.write_timestamp(&Self::M_CREATED_TIME, value)?;
        }
        if let Some(value) = self.storage_connectors.as_ref() {
            s.write_list(&Self::M_STORAGE_CONNECTORS, &|element| {
                for connector in value.iter() {
                    element.write_struct(&StorageConnector::SCHEMA, connector)?;
                }
                Ok(())
            })?;
        }
        if let Some(value) = self.redirect_url.as_ref() {
            s.write_string(&Self::M_REDIRECT_URL, value.as_str())?;
        }
        if let Some(value) = self.feedback_url.as_ref() {
            s.write_string(&Self::M_FEEDBACK_URL, value.as_str())?;
        }
        if let Some(value) = self.stack_errors.as_ref() {
            s.write_list(&Self::M_STACK_ERRORS, &|element| {
                for error in value.iter() {
                    element.write_struct(&StackError::SCHEMA, error)?;
                }
                Ok(())
            })?;
        }
        if let Some(value) = self.user_settings.as_ref() {
            s.write_list(&Self::M_USER_SETTINGS, &|element| {
                for setting in value.iter() {
                    element.write_struct(&UserSetting::SCHEMA, setting)?;
                }
                Ok(())
            })?;
        }
        if let Some(value) = self.application_settings.as_ref() {
            s.write_struct(&Self::M_APPLICATION_SETTINGS, value)?;
        }
        if let Some(value) = self.access_endpoints.as_ref() {
            s.write_list(&Self::M_ACCESS_ENDPOINTS, &|element| {
                for endpoint in value.iter() {
                    element.write_struct(&AccessEndpoint::SCHEMA, endpoint)?;
                }
                Ok(())
            })?;
        }
        if let Some(value) = self.embed_host_domains.as_ref() {
            s.write_list(&Self::M_EMBED_HOST_DOMAINS, &|element| {
                for domain in value.iter() {
                    element.write_string(&Self::M_EMBED_HOST_DOMAINS, domain.as_str())?;
                }
                Ok(())
            })?;
        }
        if let Some(value) = self.streaming_experience_settings.as_ref() {
            s.write_struct(&Self::M_STREAMING_EXPERIENCE_SETTINGS, value)?;
        }
        if let Some(value) = self.content_redirection.as_ref() {
            s.write_struct(&Self::M_CONTENT_REDIRECTION, value)?;
        }
        if let Some(value) = self.web_authn_redirection.as_ref() {
            s.write_struct(&Self::M_WEB_AUTHN_REDIRECTION, value)?;
        }
        if let Some(value) = self.agent_access_config.as_ref() {
            s.write_struct(&Self::M_AGENT_ACCESS_CONFIG, value)?;
        }
        if let Some(value) = self.storage_connector_authentication.as_ref() {
            s.write_string(&Self::M_STORAGE_CONNECTOR_AUTHENTICATION, value.as_str())?;
        }
        if let Some(value) = self.state.as_ref() {
            s.write_string(&Self::M_STATE, value.as_str())?;
        }
        Ok(())
    }
}

impl DeserializableStruct for Stack {
    fn deserialize(d: &mut dyn ShapeDeserializer) -> Result<Self, SerdeError> {
        let mut stack = Self::default();
        let mut name: Option<String> = None;
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            match member.member_index() {
                Some(0) => stack.arn = Some(d.read_string(&Self::M_ARN)?),
                Some(1) => name = Some(d.read_string(&Self::M_NAME)?),
                Some(2) => stack.description = Some(d.read_string(&Self::M_DESCRIPTION)?),
                Some(3) => stack.display_name = Some(d.read_string(&Self::M_DISPLAY_NAME)?),
                Some(4) => stack.created_time = Some(d.read_timestamp(&Self::M_CREATED_TIME)?),
                Some(5) => {
                    let mut connectors: Vec<StorageConnector> = Vec::new();
                    d.read_list(&Self::M_STORAGE_CONNECTORS, &mut |element| {
                        connectors.push(StorageConnector::deserialize(element)?);
                        Ok(())
                    })?;
                    stack.storage_connectors = Some(connectors);
                }
                Some(6) => stack.redirect_url = Some(d.read_string(&Self::M_REDIRECT_URL)?),
                Some(7) => stack.feedback_url = Some(d.read_string(&Self::M_FEEDBACK_URL)?),
                Some(8) => {
                    let mut errors: Vec<StackError> = Vec::new();
                    d.read_list(&Self::M_STACK_ERRORS, &mut |element| {
                        errors.push(StackError::deserialize(element)?);
                        Ok(())
                    })?;
                    stack.stack_errors = Some(errors);
                }
                Some(9) => {
                    let mut settings: Vec<UserSetting> = Vec::new();
                    d.read_list(&Self::M_USER_SETTINGS, &mut |element| {
                        settings.push(UserSetting::deserialize(element)?);
                        Ok(())
                    })?;
                    stack.user_settings = Some(settings);
                }
                Some(10) => {
                    stack.application_settings =
                        Some(ApplicationSettingsResponse::deserialize(d)?);
                }
                Some(11) => {
                    let mut endpoints: Vec<AccessEndpoint> = Vec::new();
                    d.read_list(&Self::M_ACCESS_ENDPOINTS, &mut |element| {
                        endpoints.push(AccessEndpoint::deserialize(element)?);
                        Ok(())
                    })?;
                    stack.access_endpoints = Some(endpoints);
                }
                Some(12) => {
                    let mut domains: Vec<String> = Vec::new();
                    d.read_list(&Self::M_EMBED_HOST_DOMAINS, &mut |element| {
                        domains.push(element.read_string(&Self::M_EMBED_HOST_DOMAINS)?);
                        Ok(())
                    })?;
                    stack.embed_host_domains = Some(domains);
                }
                Some(13) => {
                    stack.streaming_experience_settings =
                        Some(StreamingExperienceSettings::deserialize(d)?);
                }
                Some(14) => {
                    stack.content_redirection = Some(ContentRedirection::deserialize(d)?);
                }
                Some(15) => {
                    stack.web_authn_redirection = Some(WebAuthnRedirection::deserialize(d)?);
                }
                Some(16) => {
                    stack.agent_access_config = Some(AgentAccessConfig::deserialize(d)?);
                }
                Some(17) => {
                    stack.storage_connector_authentication =
                        Some(d.read_string(&Self::M_STORAGE_CONNECTOR_AUTHENTICATION)?)
                }
                Some(18) => stack.state = Some(d.read_string(&Self::M_STATE)?),
                _ => {}
            }
            Ok(())
        })?;
        stack.name = name.ok_or_else(|| ::aws_smithy_schema_server::missing_member("Name"))?;
        Ok(stack)
    }
}
