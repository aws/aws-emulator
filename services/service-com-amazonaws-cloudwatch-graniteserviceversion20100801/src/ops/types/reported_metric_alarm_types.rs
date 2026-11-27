// SPDX-License-Identifier: CC0-1.0

//! The one type every READ of a metric alarm reports an alarm through: the modelled
//! `MetricAlarm` and the `CreationId` beside it.
//!
//! HAND-WRITTEN, not rendered from the model: this module exists because the member it adds is
//! not in the model at all, so a model refresh must not be able to drop it. It lives in its own
//! file rather than in `shared_types`, which the next refresh rewrites.
//!
//! RECORDED behaviour, not modelled: `MetricAlarm` as this model declares it carries no
//! `CreationId` member
//! (`smithy_query(query="shape",shape="MetricAlarm",model_path="services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json")`),
//! while CloudWatch reports one on every read of an alarm. `#[smithy_macros::smithy(shape = …)]`
//! derives a structure's schema and codecs from the model's member list and admits exactly that
//! list, so a member the model omits cannot be declared through the derive: it is written by
//! [`ReportedMetricAlarm`] instead, the same way an alarm mute rule's `CreationId` is
//! ([`super::get_alarm_mute_rule_types::ReportedAlarmMuteRule`]). The model's own members keep
//! the indices the derive gives them, so a later model refresh that declares `CreationId` only
//! has to move the member onto `MetricAlarm` and this wrapper disappears.

use super::*;

/// The `CreationId` member of a metric alarm, appended AFTER the member list the model declares
/// for `MetricAlarm` (which occupies indices 0..=33).
const M_CREATION_ID: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
    ::aws_smithy_schema::ShapeId::from_static(
        "com.amazonaws.cloudwatch#MetricAlarm$CreationId",
        "com.amazonaws.cloudwatch",
        "CreationId",
    ),
    ::aws_smithy_schema::ShapeType::String,
    "CreationId",
    34,
);

/// One metric alarm as a READ of it reports it: the modelled [`MetricAlarm`] and the
/// `CreationId` beside it ([`M_CREATION_ID`]).
#[derive(Debug, Clone, Default)]
pub struct ReportedMetricAlarm {
    /// The alarm, as the model declares it.
    pub alarm: MetricAlarm,
    /// The `CreationId` of the write that created the alarm, absent on a read that reports
    /// less than the full alarm.
    pub creation_id: Option<String>,
}

impl ReportedMetricAlarm {
    /// The schema of the alarm this type reports, which is the MODEL's own `MetricAlarm`
    /// schema: the member beside it is written through [`M_CREATION_ID`], so the shape
    /// identity, and with it the name every protocol gives the element, stays the modelled one.
    pub const SCHEMA: ::aws_smithy_schema::Schema = MetricAlarm::SCHEMA;
}

impl ::aws_smithy_schema::serde::SerializableStruct for ReportedMetricAlarm {
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        ::aws_smithy_schema::serde::SerializableStruct::serialize_members(&self.alarm, serializer)?;
        if let Some(creation_id) = self.creation_id.as_deref() {
            serializer.write_string(&M_CREATION_ID, creation_id)?;
        }
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for ReportedMetricAlarm {
    /// Reads the modelled alarm and nothing else: `CreationId` is a member only a RESPONSE
    /// carries, so a value read off the wire has none to report.
    fn deserialize(
        deserializer: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        Ok(Self {
            alarm: <MetricAlarm as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(
                deserializer,
            )?,
            creation_id: None,
        })
    }
}
