// SPDX-License-Identifier: CC0-1.0

//! The type `GetInsightRuleReport` answers a report through: the modelled
//! `GetInsightRuleReportOutput` and the `RuleAttributes` series beside it.
//!
//! HAND-WRITTEN, not rendered from the model: the member it adds is not in the model at all, so
//! it lives in its own file rather than in a file the next model refresh rewrites.
//!
//! RECORDED behaviour, not modelled: `GetInsightRuleReportOutput` as this model declares it
//! carries no `RuleAttributes` member
//! (`smithy_query(query="operation",operation="GetInsightRuleReport",model_path="services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json")`),
//! while AWS answers this read with the series present — empty for a report that carries no rule
//! attribute. `#[smithy_macros::smithy(shape = …)]` derives a structure's schema and codecs from
//! the model's member list and admits exactly that list, so a member the model omits cannot be
//! declared through the derive: it is written by [`ReportedInsightRuleReport`] instead, the same
//! way a metric alarm's `CreationId` is
//! ([`super::reported_metric_alarm_types::ReportedMetricAlarm`]). The model's own members keep
//! the indices the derive gives them, so a later model refresh that declares `RuleAttributes`
//! only has to move the member onto `GetInsightRuleReportOutput` and this wrapper disappears.

use super::*;

/// The `RuleAttributes` member of an insight rule report, appended AFTER the member list the
/// model declares for `GetInsightRuleReportOutput` (which occupies indices 0..=5). A list of
/// strings, like the report's own `KeyLabels` series.
const M_RULE_ATTRIBUTES: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
    ::aws_smithy_schema::ShapeId::from_static(
        "com.amazonaws.cloudwatch#GetInsightRuleReportOutput$RuleAttributes",
        "com.amazonaws.cloudwatch",
        "RuleAttributes",
    ),
    ::aws_smithy_schema::ShapeType::List,
    "RuleAttributes",
    6,
);

/// One insight rule report as a READ of it reports it: the modelled
/// [`GetInsightRuleReportOutput`] and the `RuleAttributes` series beside it
/// ([`M_RULE_ATTRIBUTES`]).
#[derive(Debug, Clone, Default)]
pub struct ReportedInsightRuleReport {
    /// The report, as the model declares it.
    pub report: GetInsightRuleReportOutput,
    /// The attributes of the rule the report carries a series for: present and empty rather
    /// than absent when the report carries none, like every other collection on the answer.
    pub rule_attributes: Option<Vec<String>>,
}

impl ReportedInsightRuleReport {
    /// The schema of the report this type answers, which is the MODEL's own
    /// `GetInsightRuleReportOutput` schema: the member beside it is written through
    /// [`M_RULE_ATTRIBUTES`], so the shape identity stays the modelled one.
    pub const SCHEMA: ::aws_smithy_schema::Schema = GetInsightRuleReportOutput::SCHEMA;
}

impl ::aws_smithy_schema::serde::SerializableStruct for ReportedInsightRuleReport {
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        ::aws_smithy_schema::serde::SerializableStruct::serialize_members(&self.report, serializer)?;
        if let Some(rule_attributes) = self.rule_attributes.as_deref() {
            serializer.write_list(&M_RULE_ATTRIBUTES, &|elements| {
                for attribute in rule_attributes {
                    elements.write_string(&M_RULE_ATTRIBUTES, attribute.as_str())?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for ReportedInsightRuleReport {
    /// Reads the modelled report and nothing else: `RuleAttributes` is a member only a RESPONSE
    /// carries, so a value read off the wire has none to report.
    fn deserialize(
        deserializer: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        Ok(Self {
            report: <GetInsightRuleReportOutput as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(
                deserializer,
            )?,
            rule_attributes: None,
        })
    }
}
