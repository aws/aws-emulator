// SPDX-License-Identifier: CC0-1.0

//! The EFFECTIVE configuration-option set a read of a configuration set
//! reports, driven end to end over this service's awsQuery surface.
//!
//! RULE under test: `DescribeConfigurationSettings` reports the effective option
//! set of the platform version the configuration set is for — every option that
//! version SUPPLIES to the environment it provisions and every option its
//! published catalogue DEFINES, whoever wrote it — each at the value in force:
//! the setting the configuration itself holds, else the `DefaultValue` the
//! catalogue publishes for it, else no value at all. Beside that list, the
//! answer attributes every one of those options: `OptionSource` names the API
//! where the configuration holds the setting, the service where the platform
//! version put the value there, and NAMES NOTHING — the empty element — where no
//! value is in force.
//!
//! So the membership of the list is decided by what the platform version
//! defines, not by which of its options happen to carry a default, and a client
//! that looks an option up by (`Namespace`, `OptionName`) finds every option the
//! platform has, with the value it is running at and the source that put it
//! there.
//!
//! The catalogue each assertion is made against is read over the WIRE from
//! `DescribeConfigurationOptions` — the operation that owns it — so no option
//! name, default or namespace is restated here, and a set resolved against a
//! different catalogue than that operation publishes fails these tests.
//!
//! Each row is a different point in the class: which runtime the version runs
//! (each adds options of its own), whether the set is a saved template or a
//! running environment's configuration, and what the configuration writes — one
//! option the catalogue defines, one it defines no default for, one it does not
//! define at all, and nothing whatsoever.
//!
//! The HTTP harness (`Emulator`, `Answer`) is `all/lifecycle.rs`'s and the
//! option encoders are `all/configuration_templates.rs`'s, so nothing here
//! reaches into the store and no second copy of either can drift.

use std::collections::BTreeMap;

use crate::configuration_options::reported_options as option_descriptions;
use crate::configuration_templates::{
    as_params, created, option_params, published_stack_of_category,
};
use crate::lifecycle::{Answer, Emulator};
use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application every configuration set below belongs to.
const APPLICATION: &str = "effective-set-app";

/// The namespace of the environment properties an application reads — options
/// the CALLER names, which the published catalogue therefore does not define.
const APP_ENV: &str = "aws:elasticbeanstalk:application:environment";

/// How one option of the reported set is identified: the
/// `(Namespace, OptionName, ResourceName)` triple an `OptionSpecification` names
/// one by.
type OptionKey = (String, String, String);

/// The value in force for one reported option: `None` where the answer carries
/// no `Value` member at all.
type InForce = Option<String>;

/// What the answer reports for each option of the set, keyed by the option: the
/// value in force. Read member by member off ONE element, so an answer that
/// mixes up which value belongs to which option fails here.
fn reported_settings(answer: &Answer) -> BTreeMap<OptionKey, InForce> {
    answer
        .elements_at("OptionSettings/member")
        .iter()
        .map(|member| (option_key(member), member.value("Value").map(str::to_string)))
        .collect()
}

/// The `(Namespace, OptionName, ResourceName)` of one reported element.
fn option_key(member: &Answer) -> OptionKey {
    (
        member.value("Namespace").unwrap_or_default().to_string(),
        member.value("OptionName").unwrap_or_default().to_string(),
        member.value("ResourceName").unwrap_or_default().to_string(),
    )
}

/// What one `OptionSettingDetails` entry says about where an option's value came
/// from. An entry carrying NO `OptionSource` element at all is a different
/// answer from one carrying an element that names no source, and both are
/// asserted as such below.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReportedSource {
    /// The entry carries no `OptionSource` element.
    Absent,
    /// The element is there and names nothing — the empty element.
    NamesNothing,
    /// The element names `(Identifier, Origin)`.
    Named(String, String),
}

/// What the answer says about where each reported option's value came from: the
/// option the entry is about, and the source it names, in document order.
fn reported_details(answer: &Answer) -> Vec<(OptionKey, ReportedSource)> {
    answer
        .elements_at("OptionSettingDetails/member")
        .iter()
        .map(|member| {
            let source = if member.values_at_including_empty("OptionSource").is_empty() {
                ReportedSource::Absent
            } else {
                member
                    .elements_at("OptionSource")
                    .first()
                    .and_then(|source| {
                        let identifier = source.value("Identifier")?.to_string();
                        let origin = source.value("Origin")?.to_string();
                        Some(ReportedSource::Named(identifier, origin))
                    })
                    .unwrap_or(ReportedSource::NamesNothing)
            };
            (option_key(member), source)
        })
        .collect()
}

/// The one configuration-settings description `answer` carries.
fn only_set(answer: &Answer, case: &str) -> Answer {
    let sets = answer.elements_at("ConfigurationSettings/member");
    assert_eq!(
        sets.len(),
        1,
        "case {case:?}: expected exactly one configuration set, got {}",
        answer.body
    );
    sets.into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected one configuration set"))
}

/// Which kind of configuration set a row creates, with the action that creates
/// one and the member that names it on the create and on the read.
enum SetKind {
    /// A saved configuration: `CreateConfigurationTemplate`.
    Template,
    /// The configuration of a running environment: `CreateEnvironment`.
    Environment,
}

impl SetKind {
    fn request(&self) -> (&'static str, &'static str) {
        match self {
            Self::Template => ("CreateConfigurationTemplate", "TemplateName"),
            Self::Environment => ("CreateEnvironment", "EnvironmentName"),
        }
    }
}

/// One row: the platform version the set is for, which kind of set it is, and
/// the settings the request writes on it as `(Namespace, OptionName, Value)`.
struct EffectiveSetCase {
    case: &'static str,
    /// The `platform_category` of the published platform version, so a row names
    /// a runtime rather than a catalogue index.
    category: &'static str,
    kind: SetKind,
    name: &'static str,
    written: Vec<(&'static str, &'static str, &'static str)>,
}

#[tokio::test]
async fn a_read_reports_every_option_the_platform_version_defines_with_its_source() {
    let emulator = Emulator::new();
    created(
        &emulator,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;

    let rows = vec![
        EffectiveSetCase {
            case: "a template with nothing of its own",
            category: "Python",
            kind: SetKind::Template,
            name: "bare-template",
            written: Vec::new(),
        },
        EffectiveSetCase {
            case: "an environment that writes a defaulted option, a runtime \
                   option and a property the catalogue does not define",
            category: "Python",
            kind: SetKind::Environment,
            name: "written-env",
            written: vec![
                ("aws:autoscaling:asg", "MinSize", "3"),
                ("aws:elasticbeanstalk:container:python", "NumThreads", "25"),
                (APP_ENV, "SPRING_PROFILES_ACTIVE", "prod"),
            ],
        },
        EffectiveSetCase {
            case: "a template on another runtime that writes one of its options",
            category: "Tomcat",
            kind: SetKind::Template,
            name: "tomcat-template",
            written: vec![(
                "aws:elasticbeanstalk:container:tomcat:jvmoptions",
                "Xmx",
                "512m",
            )],
        },
        EffectiveSetCase {
            case: "an environment on a runtime that adds only the proxy option",
            category: "Go",
            kind: SetKind::Environment,
            name: "go-env",
            written: Vec::new(),
        },
        EffectiveSetCase {
            case: "a template that writes an option the catalogue gives no default for",
            category: "PHP",
            kind: SetKind::Template,
            name: "php-template",
            written: vec![("aws:ec2:vpc", "VPCId", "vpc-0abc0de1f2345678a")],
        },
    ];

    for EffectiveSetCase {
        case,
        category,
        kind,
        name,
        written,
    } in &rows
    {
        let stack = published_stack_of_category(category).name();
        // The catalogue the set resolves against, read from the operation that
        // owns it: every option this platform version publishes, with the
        // default it publishes for it where it publishes one.
        let catalogue = created(
            &emulator,
            "DescribeConfigurationOptions",
            &[("SolutionStackName", &stack)],
        )
        .await;
        let published: Vec<(OptionKey, InForce)> = option_descriptions(&catalogue)
            .into_iter()
            .map(|option| {
                (
                    (option.namespace, option.name, String::new()),
                    option.default_value,
                )
            })
            .collect();
        assert!(
            published.len() > 1,
            "case {case:?}: the published catalogue must define more than one option"
        );

        let (action, identifier) = kind.request();
        let settings: Vec<(&str, &str, &str, &str)> = written
            .iter()
            .map(|(namespace, option_name, value)| (*namespace, *option_name, *value, ""))
            .collect();
        let mut params: Vec<(String, String)> = vec![
            ("ApplicationName".to_string(), APPLICATION.to_string()),
            (identifier.to_string(), (*name).to_string()),
            ("SolutionStackName".to_string(), stack.clone()),
        ];
        params.extend(option_params(&settings));
        created(&emulator, action, &as_params(&params)).await;

        let described = created(
            &emulator,
            "DescribeConfigurationSettings",
            &[("ApplicationName", APPLICATION), (identifier, name)],
        )
        .await;
        let set = only_set(&described, case);
        let reported = reported_settings(&set);

        // Every option the platform version DEFINES is in the set, at the value
        // in force: the one this row wrote, else the published default, else no
        // value at all.
        for (key, default_value) in &published {
            let in_force = written
                .iter()
                .find(|(namespace, option_name, _)| {
                    (*namespace, *option_name) == (key.0.as_str(), key.1.as_str())
                })
                .map(|(_, _, value)| (*value).to_string())
                .or_else(|| default_value.clone());
            assert_eq!(
                reported.get(key),
                Some(&in_force),
                "case {case:?}: expected {}/{} in force at {in_force:?}, got {:?}",
                key.0,
                key.1,
                reported.get(key)
            );
        }

        // Every option the platform version SUPPLIES to the environment it
        // provisions is in the set too, even though the catalogue publishes no
        // description of it.
        for (namespace, option_name) in
            eb::state::platform_supplied_options(eb::state::solution_stack_named(&stack))
        {
            let key = (
                namespace.to_string(),
                option_name.to_string(),
                String::new(),
            );
            assert!(
                reported.contains_key(&key),
                "case {case:?}: expected the supplied {namespace}/{option_name} in the set, \
                 got {reported:?}"
            );
        }

        // An option the row wrote that the catalogue does NOT define is in the
        // set at the value it was written to.
        for (namespace, option_name, value) in written {
            let key = (
                (*namespace).to_string(),
                (*option_name).to_string(),
                String::new(),
            );
            assert_eq!(
                reported.get(&key),
                Some(&Some((*value).to_string())),
                "case {case:?}: expected the written {namespace}/{option_name} at {value:?}, \
                 got {:?}",
                reported.get(&key)
            );
        }

        // The detail list is about the SAME options, in the same order, and
        // names where each option's value came from.
        let details = reported_details(&set);
        let setting_order: Vec<OptionKey> = set
            .elements_at("OptionSettings/member")
            .iter()
            .map(option_key)
            .collect();
        let detail_order: Vec<OptionKey> = details.iter().map(|(key, _)| key.clone()).collect();
        assert_eq!(
            detail_order, setting_order,
            "case {case:?}: the detail list must name the same options in the same order, got {}",
            set.body
        );
        for (key, source) in &details {
            let written_here = written.iter().any(|(namespace, option_name, _)| {
                (*namespace, *option_name) == (key.0.as_str(), key.1.as_str())
            });
            let named = |source: &str| {
                ReportedSource::Named(source.to_string(), source.to_string())
            };
            let expected = match reported.get(key) {
                // No value in force: the source element is there and names
                // nothing.
                Some(None) => ReportedSource::NamesNothing,
                _ if written_here => named(eb::state::OPTION_SOURCE_API),
                _ => named(eb::state::OPTION_SOURCE_SERVICE),
            };
            assert_eq!(
                source,
                &expected,
                "case {case:?}: expected {}/{} attributed to {expected:?}, got {source:?}",
                key.0,
                key.1
            );
        }
    }
}
