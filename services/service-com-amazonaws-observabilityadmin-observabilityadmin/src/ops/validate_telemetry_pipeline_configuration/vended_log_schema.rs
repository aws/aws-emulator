// SPDX-License-Identifier: CC0-1.0

//! The VENDED-LOG branches of the pipeline schema: which processors a pipeline that
//! reads log events AWS itself vends may run, and the defects a configuration document
//! has against them.
//!
//! A pipeline whose records come from an AWS-vended log source runs a restricted
//! processor set: each vended source type — and, for a CloudWatch Logs source, each
//! vended data source it names — admits only the parser written for that log shape, so
//! the schema carries one BRANCH per `(source type, data source)` pair rather than one
//! open processor list. [`RESTRICTIONS`] is that table, and it lives beside the schema
//! walk in [`super`] rather than in the handler, because every operation that reports
//! defects of a configuration document walks the same schema.
//!
//! A branch is reported when the section it governs violates it. Which branches govern a
//! section comes from the section's own `source`:
//!
//! * a section declaring a vended source type is governed by that source type's
//!   branches, narrowed to the data sources it names;
//! * a section declaring NO source pins no branch at all — the schema cannot tell which
//!   vended source the document means — so every branch is reported that the section's
//!   processor sequence violates;
//! * a section declaring any other source plugin is a third-party pipeline and carries
//!   none of these restrictions.
//!
//! The defects are the restriction's own ([`REASON_DATA_SOURCE_PROCESSOR_RESTRICTED`],
//! keyed by the restriction's descriptors, because what is wrong is the pair of the
//! source and the processor rather than one field of the document) and the schema's
//! plain unknown-property one for a POSITION of the sequence naming a processor the
//! vended-log branch does not define at all ([`defines_processor`]).

use ::std::collections::HashMap;

use crate::ops::test_telemetry_pipeline::{Node, PROCESSOR_KEY, SOURCE_KEY};
use crate::ops::types::ValidationError;
use crate::state::{declared_data_source_names, plugins_declared};

use super::{
    at_most_items_message, defect_prefix, missing_property_message, processor_path,
    undefined_property_message, unknown_property,
};

/// `ValidationError$Reason` for a configuration that pairs a data source with a
/// processor the schema's branch for that source does not admit — including by running
/// more processors than the branch allows.
const REASON_DATA_SOURCE_PROCESSOR_RESTRICTED: &str = "DATA_SOURCE_PROCESSOR_RESTRICTED";

/// The `FieldMap` keys a restriction defect is reported under: the restriction's OWN
/// descriptors, since what such a defect is about is the restriction — the source type,
/// the vended data source it is about when the restriction is per data source (the
/// document's own name for it), and the processors it admits, as the one it admits or as
/// the set, joined the way the report joins a set.
const SOURCE_TYPE_KEY: &str = "source_type";
const DATA_SOURCE_NAME_KEY: &str = "data_source_name";
const ALLOWED_PROCESSOR_KEY: &str = "allowed_processor";
const ALLOWED_PROCESSORS_KEY: &str = "allowed_processors";
const ALLOWED_PROCESSOR_SEPARATOR: &str = "/";

/// One branch of the vended-log schema: the processors one AWS-vended source — or one
/// vended data source of it — admits.
struct DataSourceProcessorRestriction {
    /// The `source` plugin this branch governs, which is the token a summary reports as
    /// a `Source$Type`.
    source_type: &'static str,
    /// The vended data source this branch is about, for a source type whose
    /// restriction is per data source; `None` for a source type whose whole log shape
    /// is the one the branch restricts.
    data_source_name: Option<&'static str>,
    /// The processors this branch admits, in the order the schema lists them. Each may
    /// be named at most once, so the count is also the longest processor sequence the
    /// branch admits.
    allowed_processors: &'static [&'static str],
    /// The property the source this branch governs must declare, for a source whose log
    /// shape the pipeline cannot read without it.
    required_property: Option<&'static str>,
}

/// The vended-log restriction table.
///
/// The model states no catalogue of source plugins, data sources or processors (the
/// whole closure of `ValidateTelemetryPipelineConfiguration` exposes only
/// `ValidationError{Message,Reason,FieldMap}` —
/// `smithy_query(query="closure",operation="ValidateTelemetryPipelineConfiguration")`),
/// so this table is the service's own: the AWS-vended log sources a telemetry pipeline
/// reads, each with the parser written for that log shape and the general
/// `ocsf` normaliser. A source type named here is a vended one
/// ([`is_vended_log_source_type`]), which is also how a summary's data sources are
/// named; a recording naming another vended source type extends the table.
const RESTRICTIONS: &[DataSourceProcessorRestriction] = &[
    DataSourceProcessorRestriction {
        source_type: "cloudwatch_logs",
        data_source_name: Some("aws_cloudtrail"),
        allowed_processors: &["ocsf"],
        required_property: None,
    },
    DataSourceProcessorRestriction {
        source_type: "amazon_vpc_flow",
        data_source_name: None,
        allowed_processors: &["parse_vpc", "ocsf"],
        required_property: None,
    },
    DataSourceProcessorRestriction {
        source_type: "amazon_cloudfront",
        data_source_name: None,
        allowed_processors: &["parse_cloudfront", "ocsf"],
        // A CloudFront source reads the logs of the distributions its own `cloudfront`
        // block names, so the branch reports that property absent before it looks at
        // the processors: without it there is no log shape for them to read.
        required_property: Some("cloudfront"),
    },
    DataSourceProcessorRestriction {
        source_type: "cloudwatch_logs",
        data_source_name: Some("aws_security_hub_cspm"),
        allowed_processors: &["ocsf"],
        required_property: None,
    },
    DataSourceProcessorRestriction {
        source_type: "amazon_route53",
        data_source_name: None,
        allowed_processors: &["parse_route53", "ocsf"],
        required_property: None,
    },
];

/// The processors the vended-log branch of the pipeline schema defines BESIDE the
/// per-source parsers the restriction table names: the general parser a vended
/// CloudWatch Logs pipeline this service accepts declares.
const GENERAL_VENDED_LOG_PROCESSORS: &[&str] = &["parse_json"];

/// Whether the vended-log branch of the schema DEFINES the processor `name`: one of the
/// per-source parsers the restriction table names, or one of the general processors
/// above. Read off the table for the reason [`is_vended_log_source_type`] is, so a
/// branch added there defines its own parser without a second list to update.
///
/// A position naming anything else names a processor that schema does not define —
/// whether or not this emulator happens to run it, which is a separate observation
/// ([`super`]'s own category for a processor it cannot run).
fn defines_processor(name: &str) -> bool {
    GENERAL_VENDED_LOG_PROCESSORS.contains(&name)
        || RESTRICTIONS
            .iter()
            .any(|restriction| restriction.allowed_processors.contains(&name))
}

/// Whether `plugin` is a source plugin that reads log events AWS itself vends, as
/// opposed to a third-party source the pipeline pulls from.
///
/// Read off the restriction table, so the two never disagree: a vended source type is
/// one the schema carries a branch for. `crate::state` reports a data source by this
/// same split, because `DataSource`'s documentation splits what a data source's name and
/// type are by exactly that kind.
pub(crate) fn is_vended_log_source_type(plugin: &str) -> bool {
    RESTRICTIONS
        .iter()
        .any(|restriction| restriction.source_type == plugin)
}

/// Every defect the `sections` of a configuration document have against the vended-log
/// branches of the schema, branch by branch and then position by position.
pub(crate) fn vended_log_defects(sections: &[(&str, &Node)]) -> Vec<ValidationError> {
    let mut defects = Vec::new();
    for (name, section) in sections {
        let Some(source) = vended_source(section) else {
            continue;
        };
        let processors = ProcessorSequence::of(section);
        // These are restrictions on what a section's PROCESSORS may be, so a section
        // declaring none violates none: a pipeline that transforms nothing reads the
        // vended log events as they are.
        if processors.length == 0 {
            continue;
        }
        for restriction in RESTRICTIONS
            .iter()
            .filter(|restriction| source.governed_by(restriction))
        {
            if let Some(defect) = restriction.defect(section, &processors) {
                defects.push(defect);
            }
        }
        let prefix = defect_prefix(name);
        for (index, processor) in &processors.named {
            if !defines_processor(processor) {
                defects.push(unknown_property(
                    processor,
                    &processor_path(prefix, *index),
                ));
            }
        }
    }
    defects
}

/// What a section's `source` says about which branches govern it.
enum VendedSource<'a> {
    /// The section names no source plugin at all, so nothing pins a branch: the schema
    /// cannot tell which vended source the document means, and every branch is
    /// measured against the section.
    Unpinned,
    /// The section names one vended source type, with the data sources it declares —
    /// which narrows the branches to the ones about those data sources.
    Declared {
        source_type: &'a str,
        data_sources: Vec<&'a str>,
    },
}

impl VendedSource<'_> {
    /// Whether `restriction` is one of the branches that govern this source.
    fn governed_by(&self, restriction: &DataSourceProcessorRestriction) -> bool {
        match self {
            Self::Unpinned => true,
            Self::Declared {
                source_type,
                data_sources,
            } => {
                *source_type == restriction.source_type
                    && restriction
                        .data_source_name
                        .is_none_or(|name| data_sources.contains(&name))
            }
        }
    }
}

/// Which vended-log branches govern `section`, or `None` for a section no branch
/// governs: one reading from a third-party source plugin, and one naming more than one
/// source, whose defect is that fan-in rather than any restriction.
fn vended_source(section: &Node) -> Option<VendedSource<'_>> {
    match plugins_declared(section.get(SOURCE_KEY)).as_slice() {
        [] => Some(VendedSource::Unpinned),
        [(plugin, options)] if is_vended_log_source_type(plugin) => {
            Some(VendedSource::Declared {
                source_type: plugin,
                data_sources: declared_data_source_names(options),
            })
        }
        _ => None,
    }
}

/// A section's `processor` sequence, as a restriction measures it.
struct ProcessorSequence<'a> {
    /// How many POSITIONS the sequence has, which is what an item-count restriction
    /// counts — a position naming no processor still occupies one.
    length: usize,
    /// The processor each position that names exactly one names, with that position's
    /// index. A position naming none, or more than one, is a defect of the sequence's
    /// own form and is reported as that by the schema walk, not here.
    named: Vec<(usize, &'a str)>,
}

impl<'a> ProcessorSequence<'a> {
    fn of(section: &'a Node) -> Self {
        let Some(Node::Sequence(items)) = section.get(PROCESSOR_KEY) else {
            return Self {
                length: 0,
                named: Vec::new(),
            };
        };
        let named = items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| match item {
                Node::Scalar { text, .. } => Some((index, text.as_str())),
                _ => match item.as_mapping() {
                    Some([(name, _)]) => Some((index, name.as_str())),
                    _ => None,
                },
            })
            .collect();
        Self {
            length: items.len(),
            named,
        }
    }
}

impl DataSourceProcessorRestriction {
    /// This branch's defect for `section`, if the section violates it: more processors
    /// than the branch admits, the property the branch's source requires left out, or a
    /// position naming a processor the branch does not admit — the FIRST such position,
    /// since what the branch reports is that the section does not satisfy it.
    fn defect(
        &self,
        section: &Node,
        processors: &ProcessorSequence<'_>,
    ) -> Option<ValidationError> {
        let admitted = self.allowed_processors.len();
        if processors.length > admitted {
            return Some(self.restricted(at_most_items_message(admitted, processors.length)));
        }
        if let Some(property) = self.required_property {
            if !declares(section, property) {
                return Some(self.restricted(missing_property_message(property)));
            }
        }
        processors
            .named
            .iter()
            .find(|(_, processor)| !self.allowed_processors.contains(processor))
            .map(|(_, processor)| self.restricted(undefined_property_message(processor)))
    }

    /// One reported defect of this branch: what the section does not satisfy, under the
    /// restriction's own category, with the `FieldMap` carrying the branch's
    /// descriptors — the source type, the data source when the branch is about one, and
    /// the processor it admits or the set it admits.
    fn restricted(&self, message: String) -> ValidationError {
        let mut field_map = HashMap::from([(
            SOURCE_TYPE_KEY.to_string(),
            self.source_type.to_string(),
        )]);
        if let Some(data_source) = self.data_source_name {
            field_map.insert(DATA_SOURCE_NAME_KEY.to_string(), data_source.to_string());
        }
        match self.allowed_processors {
            [only] => field_map.insert(ALLOWED_PROCESSOR_KEY.to_string(), only.to_string()),
            several => field_map.insert(
                ALLOWED_PROCESSORS_KEY.to_string(),
                several.join(ALLOWED_PROCESSOR_SEPARATOR),
            ),
        };
        ValidationError {
            message: Some(message),
            reason: Some(REASON_DATA_SOURCE_PROCESSOR_RESTRICTED.to_string()),
            field_map: Some(field_map),
        }
    }
}

/// Whether the source `section` reads from declares `property`.
fn declares(section: &Node, property: &str) -> bool {
    plugins_declared(section.get(SOURCE_KEY))
        .iter()
        .any(|(_, options)| options.get(property).is_some())
}
