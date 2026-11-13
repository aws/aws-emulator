// SPDX-License-Identifier: CC0-1.0

//! The schema and the serializer for a response shape with a
//! `@timestampFormat("date-time")` member.
//!
//! WHY THIS MODULE EXISTS. Two timestamp shapes reach this service's responses.
//! `smithy.api#Timestamp` carries no format, so a member targeting it renders in the
//! protocol default — epoch seconds, a JSON number — which is what the Memory event
//! surfaces report. `com.amazonaws.bedrockagentcore#DateTimestamp`, which every
//! `createdAt` of the tools surfaces targets, is marked `@timestampFormat("date-time")`
//! (`smithy_query(query="shape",shape="DateTimestamp",model_path="services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json")`),
//! so on this service's restJson1 wire such a member is an RFC 3339 / ISO-8601 STRING.
//! The JSON codec decides the rendering per member from the member schema's own format
//! (`vendor/smithy-schema-server/src/json.rs`: `write_timestamp` reads
//! `Schema::timestamp_format()`), and falls back to the epoch number when the member
//! schema reports no format.
//!
//! `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`, the HTTP
//! bindings and the JSON/XML names onto each member schema it emits, but it never emits
//! `.with_timestamp_format(..)`, so a `DateTimestamp` member built by the derive went
//! onto the wire as a bare epoch number — not even the modelled JSON type, so an SDK
//! deserializing it rejects or mis-parses the response. `vendor/` is read-only from
//! inside the emulator, so the shapes whose `createdAt` or `lastUpdatedAt` targets
//! `DateTimestamp` declare
//! their schema and serializer here instead, with the one chain call the codegen omits.
//! The sibling control-plane service compensates for the same gap in its own
//! `src/ops/types/date_time_wire.rs`; when the vendored codegen learns
//! `smithy.api#timestampFormat`, delete this module and put
//! `#[smithy_macros::smithy(shape = ...)]` back on the structures below.
//!
//! WHAT IS DELIBERATELY ABSENT.
//!
//! * Constraint traits (`@length`, `@range`, …). The validating walk that enforces them
//!   runs on operation INPUTS only, and both shapes here are output-only.
//! * `with_list_member(..)`. Only the XML codecs read it, to name an element; this
//!   service's sole protocol is restJson1.
//! * `@jsonName`/`@xmlName`. No member below declares either: each one's wire name IS
//!   its Smithy member name.
//! * A `DeserializableStruct` impl. An operation's `Output` only has to be
//!   serializable (`smithy_server_runtime::Operation`), and nothing in this emulator
//!   reads a response back.
//!
//! Each member list below is the model's, member for member: same shape ids, same wire
//! names, same order (the member index is that position), same shape types.
//!
//! The PAYMENTS plane's `PaymentSession`, its `AvailableLimits` and the
//! `PaymentSessionSummary` a listing reports are here for the same reason, and they are
//! NESTED shapes rather than operation outputs: the codec renders each member from the
//! member schema of the structure that declares it, so the format has to be attached where
//! the `createdAt` / `updatedAt` member lives, not on the response that carries it. Those
//! three carry a `DeserializableStruct` impl as well, which the operation outputs that
//! nest them require of every member type the derive builds them from; nothing in this
//! emulator reads such a response back.

use ::aws_smithy_schema::serde::{
    SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer,
};
use ::aws_smithy_schema::traits::TimestampFormat;
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType};
use ::aws_smithy_schema_server::{missing_member, DeserializableStruct};

use super::{
    Amount, AvailableLimits, GetBrowserSessionOutput, GetCodeInterpreterSessionOutput,
    PaymentSession, PaymentSessionSummary, SessionLimits, StartBrowserSessionOutput,
    StartCodeInterpreterSessionOutput, StopCodeInterpreterSessionOutput,
};

/// The namespace every shape of this service lives in.
const NS: &str = "com.amazonaws.bedrockagentcore";

impl StartCodeInterpreterSessionOutput {
    pub const M_CODE_INTERPRETER_IDENTIFIER: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartCodeInterpreterSessionResponse$codeInterpreterIdentifier",
            NS,
            "codeInterpreterIdentifier",
        ),
        ShapeType::String,
        "codeInterpreterIdentifier",
        0,
    );

    pub const M_SESSION_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartCodeInterpreterSessionResponse$sessionId",
            NS,
            "sessionId",
        ),
        ShapeType::String,
        "sessionId",
        1,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_CREATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartCodeInterpreterSessionResponse$createdAt",
            NS,
            "createdAt",
        ),
        ShapeType::Timestamp,
        "createdAt",
        2,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartCodeInterpreterSessionResponse",
            NS,
            "StartCodeInterpreterSessionResponse",
        ),
        ShapeType::Structure,
        &[
            &Self::M_CODE_INTERPRETER_IDENTIFIER,
            &Self::M_SESSION_ID,
            &Self::M_CREATED_AT,
        ],
    );
}

pub static START_CODE_INTERPRETER_SESSION_OUTPUT_SCHEMA: &Schema =
    &StartCodeInterpreterSessionOutput::SCHEMA;

impl SerializableStruct for StartCodeInterpreterSessionOutput {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(
            &Self::M_CODE_INTERPRETER_IDENTIFIER,
            self.code_interpreter_identifier.as_str(),
        )?;
        s.write_string(&Self::M_SESSION_ID, self.session_id.as_str())?;
        s.write_timestamp(&Self::M_CREATED_AT, &self.created_at)?;
        Ok(())
    }
}

impl StopCodeInterpreterSessionOutput {
    pub const M_CODE_INTERPRETER_IDENTIFIER: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StopCodeInterpreterSessionResponse$codeInterpreterIdentifier",
            NS,
            "codeInterpreterIdentifier",
        ),
        ShapeType::String,
        "codeInterpreterIdentifier",
        0,
    );

    pub const M_SESSION_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StopCodeInterpreterSessionResponse$sessionId",
            NS,
            "sessionId",
        ),
        ShapeType::String,
        "sessionId",
        1,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_LAST_UPDATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StopCodeInterpreterSessionResponse$lastUpdatedAt",
            NS,
            "lastUpdatedAt",
        ),
        ShapeType::Timestamp,
        "lastUpdatedAt",
        2,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StopCodeInterpreterSessionResponse",
            NS,
            "StopCodeInterpreterSessionResponse",
        ),
        ShapeType::Structure,
        &[
            &Self::M_CODE_INTERPRETER_IDENTIFIER,
            &Self::M_SESSION_ID,
            &Self::M_LAST_UPDATED_AT,
        ],
    );
}

pub static STOP_CODE_INTERPRETER_SESSION_OUTPUT_SCHEMA: &Schema =
    &StopCodeInterpreterSessionOutput::SCHEMA;

impl SerializableStruct for StopCodeInterpreterSessionOutput {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(
            &Self::M_CODE_INTERPRETER_IDENTIFIER,
            self.code_interpreter_identifier.as_str(),
        )?;
        s.write_string(&Self::M_SESSION_ID, self.session_id.as_str())?;
        s.write_timestamp(&Self::M_LAST_UPDATED_AT, &self.last_updated_at)?;
        Ok(())
    }
}

impl GetCodeInterpreterSessionOutput {
    pub const M_CODE_INTERPRETER_IDENTIFIER: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$codeInterpreterIdentifier",
            NS,
            "codeInterpreterIdentifier",
        ),
        ShapeType::String,
        "codeInterpreterIdentifier",
        0,
    );

    pub const M_SESSION_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$sessionId",
            NS,
            "sessionId",
        ),
        ShapeType::String,
        "sessionId",
        1,
    );

    pub const M_NAME: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$name",
            NS,
            "name",
        ),
        ShapeType::String,
        "name",
        2,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_CREATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$createdAt",
            NS,
            "createdAt",
        ),
        ShapeType::Timestamp,
        "createdAt",
        3,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const M_SESSION_TIMEOUT_SECONDS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$sessionTimeoutSeconds",
            NS,
            "sessionTimeoutSeconds",
        ),
        ShapeType::Integer,
        "sessionTimeoutSeconds",
        4,
    );

    pub const M_STATUS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$status",
            NS,
            "status",
        ),
        ShapeType::String,
        "status",
        5,
    );

    pub const M_CERTIFICATES: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$certificates",
            NS,
            "certificates",
        ),
        ShapeType::List,
        "certificates",
        6,
    );

    pub const M_FILESYSTEM_CONFIGURATIONS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse$filesystemConfigurations",
            NS,
            "filesystemConfigurations",
        ),
        ShapeType::List,
        "filesystemConfigurations",
        7,
    );

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetCodeInterpreterSessionResponse",
            NS,
            "GetCodeInterpreterSessionResponse",
        ),
        ShapeType::Structure,
        &[
            &Self::M_CODE_INTERPRETER_IDENTIFIER,
            &Self::M_SESSION_ID,
            &Self::M_NAME,
            &Self::M_CREATED_AT,
            &Self::M_SESSION_TIMEOUT_SECONDS,
            &Self::M_STATUS,
            &Self::M_CERTIFICATES,
            &Self::M_FILESYSTEM_CONFIGURATIONS,
        ],
    );
}

pub static GET_CODE_INTERPRETER_SESSION_OUTPUT_SCHEMA: &Schema =
    &GetCodeInterpreterSessionOutput::SCHEMA;

impl SerializableStruct for GetCodeInterpreterSessionOutput {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(
            &Self::M_CODE_INTERPRETER_IDENTIFIER,
            self.code_interpreter_identifier.as_str(),
        )?;
        s.write_string(&Self::M_SESSION_ID, self.session_id.as_str())?;
        if let Some(name) = self.name.as_ref() {
            s.write_string(&Self::M_NAME, name.as_str())?;
        }
        s.write_timestamp(&Self::M_CREATED_AT, &self.created_at)?;
        if let Some(timeout) = self.session_timeout_seconds {
            s.write_integer(&Self::M_SESSION_TIMEOUT_SECONDS, timeout)?;
        }
        if let Some(status) = self.status.as_ref() {
            s.write_string(&Self::M_STATUS, status.as_str())?;
        }
        if let Some(certificates) = self.certificates.as_ref() {
            s.write_list(&Self::M_CERTIFICATES, &|list| {
                for certificate in certificates {
                    list.write_struct(&super::Certificate::SCHEMA, certificate)?;
                }
                Ok(())
            })?;
        }
        if let Some(configurations) = self.filesystem_configurations.as_ref() {
            s.write_list(&Self::M_FILESYSTEM_CONFIGURATIONS, &|list| {
                for configuration in configurations {
                    list.write_struct(
                        &super::ToolsFileSystemConfiguration::SCHEMA,
                        configuration,
                    )?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}

impl StartBrowserSessionOutput {
    pub const M_BROWSER_IDENTIFIER: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartBrowserSessionResponse$browserIdentifier",
            NS,
            "browserIdentifier",
        ),
        ShapeType::String,
        "browserIdentifier",
        0,
    );

    pub const M_SESSION_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartBrowserSessionResponse$sessionId",
            NS,
            "sessionId",
        ),
        ShapeType::String,
        "sessionId",
        1,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_CREATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartBrowserSessionResponse$createdAt",
            NS,
            "createdAt",
        ),
        ShapeType::Timestamp,
        "createdAt",
        2,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const M_STREAMS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartBrowserSessionResponse$streams",
            NS,
            "streams",
        ),
        ShapeType::Structure,
        "streams",
        3,
    );

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#StartBrowserSessionResponse",
            NS,
            "StartBrowserSessionResponse",
        ),
        ShapeType::Structure,
        &[
            &Self::M_BROWSER_IDENTIFIER,
            &Self::M_SESSION_ID,
            &Self::M_CREATED_AT,
            &Self::M_STREAMS,
        ],
    );
}

pub static START_BROWSER_SESSION_OUTPUT_SCHEMA: &Schema = &StartBrowserSessionOutput::SCHEMA;

impl SerializableStruct for StartBrowserSessionOutput {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&Self::M_BROWSER_IDENTIFIER, self.browser_identifier.as_str())?;
        s.write_string(&Self::M_SESSION_ID, self.session_id.as_str())?;
        s.write_timestamp(&Self::M_CREATED_AT, &self.created_at)?;
        if let Some(streams) = self.streams.as_ref() {
            s.write_struct(&Self::M_STREAMS, streams)?;
        }
        Ok(())
    }
}

impl GetBrowserSessionOutput {
    pub const M_BROWSER_IDENTIFIER: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$browserIdentifier",
            NS,
            "browserIdentifier",
        ),
        ShapeType::String,
        "browserIdentifier",
        0,
    );

    pub const M_SESSION_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$sessionId",
            NS,
            "sessionId",
        ),
        ShapeType::String,
        "sessionId",
        1,
    );

    pub const M_NAME: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$name",
            NS,
            "name",
        ),
        ShapeType::String,
        "name",
        2,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_CREATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$createdAt",
            NS,
            "createdAt",
        ),
        ShapeType::Timestamp,
        "createdAt",
        3,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const M_VIEW_PORT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$viewPort",
            NS,
            "viewPort",
        ),
        ShapeType::Structure,
        "viewPort",
        4,
    );

    pub const M_EXTENSIONS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$extensions",
            NS,
            "extensions",
        ),
        ShapeType::List,
        "extensions",
        5,
    );

    pub const M_ENTERPRISE_POLICIES: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$enterprisePolicies",
            NS,
            "enterprisePolicies",
        ),
        ShapeType::List,
        "enterprisePolicies",
        6,
    );

    pub const M_PROFILE_CONFIGURATION: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$profileConfiguration",
            NS,
            "profileConfiguration",
        ),
        ShapeType::Structure,
        "profileConfiguration",
        7,
    );

    pub const M_SESSION_TIMEOUT_SECONDS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$sessionTimeoutSeconds",
            NS,
            "sessionTimeoutSeconds",
        ),
        ShapeType::Integer,
        "sessionTimeoutSeconds",
        8,
    );

    pub const M_STATUS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$status",
            NS,
            "status",
        ),
        ShapeType::String,
        "status",
        9,
    );

    pub const M_STREAMS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$streams",
            NS,
            "streams",
        ),
        ShapeType::Structure,
        "streams",
        10,
    );

    pub const M_PROXY_CONFIGURATION: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$proxyConfiguration",
            NS,
            "proxyConfiguration",
        ),
        ShapeType::Structure,
        "proxyConfiguration",
        11,
    );

    pub const M_CERTIFICATES: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$certificates",
            NS,
            "certificates",
        ),
        ShapeType::List,
        "certificates",
        12,
    );

    pub const M_FILESYSTEM_CONFIGURATIONS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$filesystemConfigurations",
            NS,
            "filesystemConfigurations",
        ),
        ShapeType::List,
        "filesystemConfigurations",
        13,
    );

    pub const M_SESSION_REPLAY_ARTIFACT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$sessionReplayArtifact",
            NS,
            "sessionReplayArtifact",
        ),
        ShapeType::String,
        "sessionReplayArtifact",
        14,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_LAST_UPDATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse$lastUpdatedAt",
            NS,
            "lastUpdatedAt",
        ),
        ShapeType::Timestamp,
        "lastUpdatedAt",
        15,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#GetBrowserSessionResponse",
            NS,
            "GetBrowserSessionResponse",
        ),
        ShapeType::Structure,
        &[
            &Self::M_BROWSER_IDENTIFIER,
            &Self::M_SESSION_ID,
            &Self::M_NAME,
            &Self::M_CREATED_AT,
            &Self::M_VIEW_PORT,
            &Self::M_EXTENSIONS,
            &Self::M_ENTERPRISE_POLICIES,
            &Self::M_PROFILE_CONFIGURATION,
            &Self::M_SESSION_TIMEOUT_SECONDS,
            &Self::M_STATUS,
            &Self::M_STREAMS,
            &Self::M_PROXY_CONFIGURATION,
            &Self::M_CERTIFICATES,
            &Self::M_FILESYSTEM_CONFIGURATIONS,
            &Self::M_SESSION_REPLAY_ARTIFACT,
            &Self::M_LAST_UPDATED_AT,
        ],
    );
}

pub static GET_BROWSER_SESSION_OUTPUT_SCHEMA: &Schema = &GetBrowserSessionOutput::SCHEMA;

impl SerializableStruct for GetBrowserSessionOutput {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&Self::M_BROWSER_IDENTIFIER, self.browser_identifier.as_str())?;
        s.write_string(&Self::M_SESSION_ID, self.session_id.as_str())?;
        if let Some(name) = self.name.as_ref() {
            s.write_string(&Self::M_NAME, name.as_str())?;
        }
        s.write_timestamp(&Self::M_CREATED_AT, &self.created_at)?;
        if let Some(view_port) = self.view_port.as_ref() {
            s.write_struct(&Self::M_VIEW_PORT, view_port)?;
        }
        if let Some(extensions) = self.extensions.as_ref() {
            s.write_list(&Self::M_EXTENSIONS, &|list| {
                for extension in extensions {
                    list.write_struct(&super::BrowserExtension::SCHEMA, extension)?;
                }
                Ok(())
            })?;
        }
        if let Some(policies) = self.enterprise_policies.as_ref() {
            s.write_list(&Self::M_ENTERPRISE_POLICIES, &|list| {
                for policy in policies {
                    list.write_struct(&super::BrowserEnterprisePolicy::SCHEMA, policy)?;
                }
                Ok(())
            })?;
        }
        if let Some(profile) = self.profile_configuration.as_ref() {
            s.write_struct(&Self::M_PROFILE_CONFIGURATION, profile)?;
        }
        if let Some(timeout) = self.session_timeout_seconds {
            s.write_integer(&Self::M_SESSION_TIMEOUT_SECONDS, timeout)?;
        }
        if let Some(status) = self.status.as_ref() {
            s.write_string(&Self::M_STATUS, status.as_str())?;
        }
        if let Some(streams) = self.streams.as_ref() {
            s.write_struct(&Self::M_STREAMS, streams)?;
        }
        if let Some(proxy) = self.proxy_configuration.as_ref() {
            s.write_struct(&Self::M_PROXY_CONFIGURATION, proxy)?;
        }
        if let Some(certificates) = self.certificates.as_ref() {
            s.write_list(&Self::M_CERTIFICATES, &|list| {
                for certificate in certificates {
                    list.write_struct(&super::Certificate::SCHEMA, certificate)?;
                }
                Ok(())
            })?;
        }
        if let Some(configurations) = self.filesystem_configurations.as_ref() {
            s.write_list(&Self::M_FILESYSTEM_CONFIGURATIONS, &|list| {
                for configuration in configurations {
                    list.write_struct(
                        &super::ToolsFileSystemConfiguration::SCHEMA,
                        configuration,
                    )?;
                }
                Ok(())
            })?;
        }
        if let Some(artifact) = self.session_replay_artifact.as_ref() {
            s.write_string(&Self::M_SESSION_REPLAY_ARTIFACT, artifact.as_str())?;
        }
        if let Some(last_updated_at) = self.last_updated_at.as_ref() {
            s.write_timestamp(&Self::M_LAST_UPDATED_AT, last_updated_at)?;
        }
        Ok(())
    }
}

impl AvailableLimits {
    pub const M_AVAILABLE_SPEND_AMOUNT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#AvailableLimits$availableSpendAmount",
            NS,
            "availableSpendAmount",
        ),
        ShapeType::Structure,
        "availableSpendAmount",
        0,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_UPDATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#AvailableLimits$updatedAt",
            NS,
            "updatedAt",
        ),
        ShapeType::Timestamp,
        "updatedAt",
        1,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#AvailableLimits",
            NS,
            "AvailableLimits",
        ),
        ShapeType::Structure,
        &[&Self::M_AVAILABLE_SPEND_AMOUNT, &Self::M_UPDATED_AT],
    );
}

pub static AVAILABLE_LIMITS_SCHEMA: &Schema = &AvailableLimits::SCHEMA;

impl SerializableStruct for AvailableLimits {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        if let Some(amount) = self.available_spend_amount.as_ref() {
            s.write_struct(&Self::M_AVAILABLE_SPEND_AMOUNT, amount)?;
        }
        if let Some(updated_at) = self.updated_at.as_ref() {
            s.write_timestamp(&Self::M_UPDATED_AT, updated_at)?;
        }
        Ok(())
    }
}

impl DeserializableStruct for AvailableLimits {
    fn deserialize(d: &mut dyn ShapeDeserializer) -> Result<Self, SerdeError> {
        let mut available_spend_amount = None;
        let mut updated_at = None;
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            match member.member_index() {
                Some(0) => available_spend_amount = Some(Amount::deserialize(d)?),
                Some(1) => updated_at = Some(d.read_timestamp(&Self::M_UPDATED_AT)?),
                _ => {}
            }
            Ok(())
        })?;
        Ok(Self {
            available_spend_amount,
            updated_at,
        })
    }
}

impl PaymentSession {
    pub const M_PAYMENT_SESSION_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSession$paymentSessionId",
            NS,
            "paymentSessionId",
        ),
        ShapeType::String,
        "paymentSessionId",
        0,
    );

    pub const M_PAYMENT_MANAGER_ARN: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSession$paymentManagerArn",
            NS,
            "paymentManagerArn",
        ),
        ShapeType::String,
        "paymentManagerArn",
        1,
    );

    pub const M_LIMITS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSession$limits",
            NS,
            "limits",
        ),
        ShapeType::Structure,
        "limits",
        2,
    );

    pub const M_USER_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSession$userId",
            NS,
            "userId",
        ),
        ShapeType::String,
        "userId",
        3,
    );

    pub const M_EXPIRY_TIME_IN_MINUTES: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSession$expiryTimeInMinutes",
            NS,
            "expiryTimeInMinutes",
        ),
        ShapeType::Integer,
        "expiryTimeInMinutes",
        4,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_CREATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSession$createdAt",
            NS,
            "createdAt",
        ),
        ShapeType::Timestamp,
        "createdAt",
        5,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const M_AVAILABLE_LIMITS: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSession$availableLimits",
            NS,
            "availableLimits",
        ),
        ShapeType::Structure,
        "availableLimits",
        6,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_UPDATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSession$updatedAt",
            NS,
            "updatedAt",
        ),
        ShapeType::Timestamp,
        "updatedAt",
        7,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSession",
            NS,
            "PaymentSession",
        ),
        ShapeType::Structure,
        &[
            &Self::M_PAYMENT_SESSION_ID,
            &Self::M_PAYMENT_MANAGER_ARN,
            &Self::M_LIMITS,
            &Self::M_USER_ID,
            &Self::M_EXPIRY_TIME_IN_MINUTES,
            &Self::M_CREATED_AT,
            &Self::M_AVAILABLE_LIMITS,
            &Self::M_UPDATED_AT,
        ],
    );
}

pub static PAYMENT_SESSION_SCHEMA: &Schema = &PaymentSession::SCHEMA;

impl SerializableStruct for PaymentSession {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&Self::M_PAYMENT_SESSION_ID, self.payment_session_id.as_str())?;
        s.write_string(
            &Self::M_PAYMENT_MANAGER_ARN,
            self.payment_manager_arn.as_str(),
        )?;
        if let Some(limits) = self.limits.as_ref() {
            s.write_struct(&Self::M_LIMITS, limits)?;
        }
        s.write_string(&Self::M_USER_ID, self.user_id.as_str())?;
        s.write_integer(&Self::M_EXPIRY_TIME_IN_MINUTES, self.expiry_time_in_minutes)?;
        s.write_timestamp(&Self::M_CREATED_AT, &self.created_at)?;
        if let Some(available_limits) = self.available_limits.as_ref() {
            s.write_struct(&Self::M_AVAILABLE_LIMITS, available_limits)?;
        }
        s.write_timestamp(&Self::M_UPDATED_AT, &self.updated_at)?;
        Ok(())
    }
}

impl DeserializableStruct for PaymentSession {
    fn deserialize(d: &mut dyn ShapeDeserializer) -> Result<Self, SerdeError> {
        let mut payment_session_id = None;
        let mut payment_manager_arn = None;
        let mut limits = None;
        let mut user_id = None;
        let mut expiry_time_in_minutes = None;
        let mut created_at = None;
        let mut available_limits = None;
        let mut updated_at = None;
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            match member.member_index() {
                Some(0) => {
                    payment_session_id = Some(d.read_string(&Self::M_PAYMENT_SESSION_ID)?);
                }
                Some(1) => {
                    payment_manager_arn = Some(d.read_string(&Self::M_PAYMENT_MANAGER_ARN)?);
                }
                Some(2) => limits = Some(SessionLimits::deserialize(d)?),
                Some(3) => user_id = Some(d.read_string(&Self::M_USER_ID)?),
                Some(4) => {
                    expiry_time_in_minutes =
                        Some(d.read_integer(&Self::M_EXPIRY_TIME_IN_MINUTES)?);
                }
                Some(5) => created_at = Some(d.read_timestamp(&Self::M_CREATED_AT)?),
                Some(6) => available_limits = Some(AvailableLimits::deserialize(d)?),
                Some(7) => updated_at = Some(d.read_timestamp(&Self::M_UPDATED_AT)?),
                _ => {}
            }
            Ok(())
        })?;
        Ok(Self {
            payment_session_id: payment_session_id
                .ok_or_else(|| missing_member("paymentSessionId"))?,
            payment_manager_arn: payment_manager_arn
                .ok_or_else(|| missing_member("paymentManagerArn"))?,
            limits,
            user_id: user_id.ok_or_else(|| missing_member("userId"))?,
            expiry_time_in_minutes: expiry_time_in_minutes
                .ok_or_else(|| missing_member("expiryTimeInMinutes"))?,
            created_at: created_at.ok_or_else(|| missing_member("createdAt"))?,
            available_limits,
            updated_at: updated_at.ok_or_else(|| missing_member("updatedAt"))?,
        })
    }
}

impl PaymentSessionSummary {
    pub const M_PAYMENT_SESSION_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSessionSummary$paymentSessionId",
            NS,
            "paymentSessionId",
        ),
        ShapeType::String,
        "paymentSessionId",
        0,
    );

    pub const M_PAYMENT_MANAGER_ARN: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSessionSummary$paymentManagerArn",
            NS,
            "paymentManagerArn",
        ),
        ShapeType::String,
        "paymentManagerArn",
        1,
    );

    pub const M_USER_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSessionSummary$userId",
            NS,
            "userId",
        ),
        ShapeType::String,
        "userId",
        2,
    );

    pub const M_EXPIRY_TIME_IN_MINUTES: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSessionSummary$expiryTimeInMinutes",
            NS,
            "expiryTimeInMinutes",
        ),
        ShapeType::Integer,
        "expiryTimeInMinutes",
        3,
    );

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_CREATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSessionSummary$createdAt",
            NS,
            "createdAt",
        ),
        ShapeType::Timestamp,
        "createdAt",
        4,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    /// Targets `DateTimestamp`, hence the format the derive cannot attach.
    pub const M_UPDATED_AT: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSessionSummary$updatedAt",
            NS,
            "updatedAt",
        ),
        ShapeType::Timestamp,
        "updatedAt",
        5,
    )
    .with_timestamp_format(TimestampFormat::DateTime);

    pub const SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "com.amazonaws.bedrockagentcore#PaymentSessionSummary",
            NS,
            "PaymentSessionSummary",
        ),
        ShapeType::Structure,
        &[
            &Self::M_PAYMENT_SESSION_ID,
            &Self::M_PAYMENT_MANAGER_ARN,
            &Self::M_USER_ID,
            &Self::M_EXPIRY_TIME_IN_MINUTES,
            &Self::M_CREATED_AT,
            &Self::M_UPDATED_AT,
        ],
    );
}

pub static PAYMENT_SESSION_SUMMARY_SCHEMA: &Schema = &PaymentSessionSummary::SCHEMA;

impl SerializableStruct for PaymentSessionSummary {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&Self::M_PAYMENT_SESSION_ID, self.payment_session_id.as_str())?;
        s.write_string(
            &Self::M_PAYMENT_MANAGER_ARN,
            self.payment_manager_arn.as_str(),
        )?;
        s.write_string(&Self::M_USER_ID, self.user_id.as_str())?;
        s.write_integer(&Self::M_EXPIRY_TIME_IN_MINUTES, self.expiry_time_in_minutes)?;
        s.write_timestamp(&Self::M_CREATED_AT, &self.created_at)?;
        s.write_timestamp(&Self::M_UPDATED_AT, &self.updated_at)?;
        Ok(())
    }
}

impl DeserializableStruct for PaymentSessionSummary {
    fn deserialize(d: &mut dyn ShapeDeserializer) -> Result<Self, SerdeError> {
        let mut payment_session_id = None;
        let mut payment_manager_arn = None;
        let mut user_id = None;
        let mut expiry_time_in_minutes = None;
        let mut created_at = None;
        let mut updated_at = None;
        d.read_struct(&Self::SCHEMA, &mut |member, d| {
            match member.member_index() {
                Some(0) => {
                    payment_session_id = Some(d.read_string(&Self::M_PAYMENT_SESSION_ID)?);
                }
                Some(1) => {
                    payment_manager_arn = Some(d.read_string(&Self::M_PAYMENT_MANAGER_ARN)?);
                }
                Some(2) => user_id = Some(d.read_string(&Self::M_USER_ID)?),
                Some(3) => {
                    expiry_time_in_minutes =
                        Some(d.read_integer(&Self::M_EXPIRY_TIME_IN_MINUTES)?);
                }
                Some(4) => created_at = Some(d.read_timestamp(&Self::M_CREATED_AT)?),
                Some(5) => updated_at = Some(d.read_timestamp(&Self::M_UPDATED_AT)?),
                _ => {}
            }
            Ok(())
        })?;
        Ok(Self {
            payment_session_id: payment_session_id
                .ok_or_else(|| missing_member("paymentSessionId"))?,
            payment_manager_arn: payment_manager_arn
                .ok_or_else(|| missing_member("paymentManagerArn"))?,
            user_id: user_id.ok_or_else(|| missing_member("userId"))?,
            expiry_time_in_minutes: expiry_time_in_minutes
                .ok_or_else(|| missing_member("expiryTimeInMinutes"))?,
            created_at: created_at.ok_or_else(|| missing_member("createdAt"))?,
            updated_at: updated_at.ok_or_else(|| missing_member("updatedAt"))?,
        })
    }
}
