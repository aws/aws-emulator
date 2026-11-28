// SPDX-License-Identifier: CC0-1.0

//! Configuration RESOLUTION: what a read of a configuration set reports, driven
//! end to end over this service's awsQuery surface.
//!
//! RULE under test: a configuration set reports the RESOLVED option settings of
//! the platform version it is for — the options that version SUPPLIES to the
//! environment it provisions and every option its published catalogue defines,
//! each at the value in force: the setting the configuration itself holds, else
//! the `DefaultValue` the catalogue gives for it, else no value at all. So the
//! set a read answers with carries options the caller never wrote, a supplied
//! option the service puts no value in force for is reported with no value, an
//! option the catalogue gives no default for is reported at no value until the
//! configuration sets one, and an option an `OptionsToRemove` takes out stops
//! overriding its default and resolves back to it instead of vanishing from the
//! set.
//!
//! The defaults every assertion here is made against are read over the WIRE from
//! `DescribeConfigurationOptions` — the operation that owns the catalogue — so
//! nothing below restates a default value, and a configuration set resolved
//! against a different catalogue than the one that operation publishes fails
//! these tests.
//!
//! Each table row is a different point in that class: which platform version the
//! set is for (its runtime adds options of its own), whether the set is a saved
//! template or a running environment's configuration, how many settings it
//! holds, whether a setting overrides an option the catalogue defines or names
//! one it does not, whether it carries a `ResourceName`, and which kind of
//! option a removal names.
//!
//! The HTTP harness (`Emulator`, `Answer`) is the one `all/lifecycle.rs` owns and
//! the option encoders and readers are `all/configuration_templates.rs`'s, so no
//! second copy of either can drift. Nothing here reaches into the store.

use std::collections::BTreeMap;

use crate::configuration_options::{reported_options as option_descriptions, ReportedOption};
use crate::configuration_templates::{
    as_params, created, option_params, published_stack_of_category, remove_params,
    reported_options as reported_settings,
};
use crate::lifecycle::{wait_for_status, Answer, Emulator};
use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application every configuration set below belongs to.
const APPLICATION: &str = "resolution-app";

/// The Auto Scaling group namespace, which holds the two sibling options the
/// reported case is about: one the configuration writes, one it never does.
const ASG: &str = "aws:autoscaling:asg";
/// The namespace of the environment variables an application reads — options the
/// CALLER names, which the published catalogue therefore does not define.
const APP_ENV: &str = "aws:elasticbeanstalk:application:environment";
/// A scheduled scaling action: the namespace whose settings carry a
/// `ResourceName` of their own.
const SCHEDULED: &str = "aws:autoscaling:scheduledaction";

/// One option setting as `(Namespace, OptionName, Value, ResourceName)`, OWNED,
/// so a row can carry a value derived from the catalogue the service publishes.
type Setting = (String, String, String, String);

/// One owned setting.
fn setting(namespace: &str, option_name: &str, value: &str, resource_name: &str) -> Setting {
    (
        namespace.to_string(),
        option_name.to_string(),
        value.to_string(),
        resource_name.to_string(),
    )
}

/// `settings` in the borrowed shape the shared encoder takes.
fn borrowed(settings: &[Setting]) -> Vec<(&str, &str, &str, &str)> {
    settings
        .iter()
        .map(|(namespace, option_name, value, resource_name)| {
            (
                namespace.as_str(),
                option_name.as_str(),
                value.as_str(),
                resource_name.as_str(),
            )
        })
        .collect()
}

/// The option catalogue `DescribeConfigurationOptions` publishes for `stack`, as
/// `(Namespace, OptionName) -> DefaultValue` for EVERY option it publishes — the
/// empty string for one it publishes no `DefaultValue` for, which is the value
/// in force for it: none.
///
/// This is the catalogue a configuration set for that platform version resolves
/// against, read from the operation that owns it rather than from a list written
/// here.
async fn published_defaults(
    emulator: &Emulator,
    stack: &str,
) -> BTreeMap<(String, String), String> {
    let answer = created(
        emulator,
        "DescribeConfigurationOptions",
        &[("SolutionStackName", stack)],
    )
    .await;
    option_descriptions(&answer)
        .into_iter()
        .map(|option| {
            (
                (option.namespace, option.name),
                option.default_value.unwrap_or_default(),
            )
        })
        .collect()
}

/// The first option `DescribeConfigurationOptions` publishes for `stack` that
/// `wanted` accepts, as `(Namespace, OptionName)`.
///
/// The catalogue is what says which option is of the KIND a test is about — one
/// it gives no default for, say — so no test names such an option itself.
async fn published_option_named(
    emulator: &Emulator,
    stack: &str,
    case: &str,
    wanted: impl Fn(&ReportedOption) -> bool,
) -> (String, String) {
    let answer = created(
        emulator,
        "DescribeConfigurationOptions",
        &[("SolutionStackName", stack)],
    )
    .await;
    let found = option_descriptions(&answer)
        .into_iter()
        .find(|option| wanted(option))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the published catalogue has no option of this kind")
        });
    (found.namespace, found.name)
}

/// The configuration set `written` puts in force on the platform version
/// `stack`, whose published options are `defaults`: the options that version
/// SUPPLIES, then every published option at the default the catalogue gives it,
/// with the written settings over them, sorted by the key an option is
/// identified by.
///
/// The supplied options are the ones the platform version puts in the set
/// without publishing a configurable description of them — the parameters of the
/// environment the service provisions — so WHICH they are is read from the
/// service (`eb::state::platform_supplied_options`) rather than listed here.
/// Each is in force at no value, reported with no `Value` (which the reader
/// answers as the empty string), except the instance-type FAMILY, which the
/// service derives from the instance type the configuration runs on, and the
/// module search path a Python runtime points at the directory it runs the
/// deployed application from.
fn expected_resolved(
    stack: &str,
    defaults: &BTreeMap<(String, String), String>,
    written: &[Setting],
) -> Vec<Setting> {
    let published = eb::state::solution_stack_named(stack);
    let family = expected_instance_type_family(defaults, written);
    let mut resolved: BTreeMap<(String, String, String), String> =
        eb::state::platform_supplied_options(published)
            .into_iter()
            .map(|(namespace, option_name)| {
                let value = match option_name {
                    INSTANCE_TYPE_FAMILY => family.clone(),
                    eb::state::PYTHON_MODULE_SEARCH_PATH => {
                        eb::state::DEPLOYED_APPLICATION_DIRECTORY.to_string()
                    }
                    _ => String::new(),
                };
                (
                    (namespace.to_string(), option_name.to_string(), String::new()),
                    value,
                )
            })
            .collect();
    resolved.extend(defaults.iter().map(|((namespace, option_name), value)| {
        (
            (namespace.clone(), option_name.clone(), String::new()),
            value.clone(),
        )
    }));
    for (namespace, option_name, value, resource_name) in written {
        resolved.insert(
            (
                namespace.clone(),
                option_name.clone(),
                resource_name.clone(),
            ),
            value.clone(),
        );
    }
    let mut rows: Vec<Setting> = resolved
        .into_iter()
        .map(|((namespace, option_name, resource_name), value)| {
            (namespace, option_name, value, resource_name)
        })
        .collect();
    rows.sort();
    rows
}

/// The template parameter the service puts the instance type's FAMILY in force
/// as, and the launch-configuration option naming that type.
const INSTANCE_TYPE_FAMILY: &str = "InstanceTypeFamily";
/// See [`INSTANCE_TYPE_FAMILY`].
const LAUNCH_CONFIGURATION: &str = "aws:autoscaling:launchconfiguration";
/// See [`INSTANCE_TYPE_FAMILY`].
const INSTANCE_TYPE: &str = "InstanceType";

/// The value the instance-type family is in force at: the part before the size
/// of the instance type the configuration runs on — the one the configuration
/// wrote, else the default the platform version publishes for it, else the type
/// an environment that names none runs.
fn expected_instance_type_family(
    defaults: &BTreeMap<(String, String), String>,
    written: &[Setting],
) -> String {
    let instance_type = written
        .iter()
        .find(|(namespace, option_name, _, _)| {
            namespace == LAUNCH_CONFIGURATION && option_name == INSTANCE_TYPE
        })
        .map(|(_, _, value, _)| value.clone())
        .or_else(|| {
            defaults
                .get(&(LAUNCH_CONFIGURATION.to_string(), INSTANCE_TYPE.to_string()))
                .cloned()
        })
        .unwrap_or_else(|| eb::state::DEFAULT_INSTANCE_TYPE.to_string());
    instance_type
        .split_once('.')
        .map_or(instance_type.clone(), |(family, _)| family.to_string())
}

/// The option settings `answer` reports, sorted the way [`expected_resolved`]
/// answers.
fn resolved_settings(answer: &Answer) -> Vec<Setting> {
    let mut reported = reported_settings(answer);
    reported.sort();
    reported
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

/// Which kind of configuration set a row creates.
enum SetKind {
    /// A saved configuration: `CreateConfigurationTemplate`.
    Template,
    /// The configuration of a running environment: `CreateEnvironment`.
    Environment,
}

impl SetKind {
    /// The create action, and the member that names the set on both the create
    /// and the read.
    fn request(&self) -> (&'static str, &'static str) {
        match self {
            Self::Template => ("CreateConfigurationTemplate", "TemplateName"),
            Self::Environment => ("CreateEnvironment", "EnvironmentName"),
        }
    }
}

/// One row of the resolution table: the platform version the set is for, which
/// kind of set it is, and the settings the request writes on it.
struct ResolutionCase {
    case: &'static str,
    /// The `platform_category` of the published platform version the set is
    /// for, so a row names a runtime rather than a catalogue index.
    category: &'static str,
    kind: SetKind,
    name: &'static str,
    written: Vec<Setting>,
}

#[tokio::test]
async fn a_configuration_set_reports_the_published_defaults_with_its_own_settings_over_them() {
    let emulator = Emulator::new();
    created(
        &emulator,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;

    // The override the reported case writes: the published default for
    // `MinSize` plus two, so the value written cannot be mistaken for the
    // default it replaces.
    let python = published_stack_of_category("Python").name();
    let python_defaults = published_defaults(&emulator, &python).await;
    let min_size_default = python_defaults
        .get(&(ASG.to_string(), "MinSize".to_string()))
        .expect("the published catalogue gives aws:autoscaling:asg/MinSize a default")
        .parse::<i64>()
        .expect("aws:autoscaling:asg/MinSize's published default is numeric");
    let raised_min_size = (min_size_default + 2).to_string();

    let rows = vec![
        ResolutionCase {
            // The reported case: ONE option written, and the sibling the caller
            // never wrote resolving to its published default.
            case: "a template with one override of a published option",
            category: "Python",
            kind: SetKind::Template,
            name: "one-override",
            written: vec![setting(ASG, "MinSize", &raised_min_size, "")],
        },
        ResolutionCase {
            // Another runtime: its catalogue carries options the one above does
            // not, so a set resolved against the wrong catalogue fails here.
            case: "a template on another runtime, with several settings",
            category: "Tomcat",
            kind: SetKind::Template,
            name: "several-settings",
            written: vec![
                setting(ASG, "MaxSize", "9", ""),
                setting(APP_ENV, "STAGE", "qa", ""),
                setting(SCHEDULED, "MinSize", "4", "peak-hours"),
            ],
        },
        ResolutionCase {
            // Nothing written at all: the resolved set is the catalogue's
            // defaults alone, which is still not an empty set.
            case: "a template with no settings of its own",
            category: "PHP",
            kind: SetKind::Template,
            name: "no-settings",
            written: Vec::new(),
        },
        ResolutionCase {
            // A setting written at EXACTLY the published default: the set still
            // reports that option, at that value, so a configuration that
            // restates a default is neither dropped nor doubled.
            case: "a template that writes an option at its published default",
            category: "Python",
            kind: SetKind::Template,
            name: "writes-the-default",
            written: vec![setting(ASG, "MinSize", &min_size_default.to_string(), "")],
        },
        ResolutionCase {
            case: "a running environment's configuration",
            category: "Python",
            kind: SetKind::Environment,
            name: "resolved-env",
            written: vec![setting(ASG, "MinSize", &raised_min_size, "")],
        },
        ResolutionCase {
            case: "a running environment launched with no settings",
            category: "Tomcat",
            kind: SetKind::Environment,
            name: "bare-env",
            written: Vec::new(),
        },
    ];

    for ResolutionCase {
        case,
        category,
        kind,
        name,
        written,
    } in &rows
    {
        let stack = published_stack_of_category(category).name();
        let defaults = published_defaults(&emulator, &stack).await;

        let (action, identifier) = kind.request();
        let mut params: Vec<(String, String)> = vec![
            ("ApplicationName".to_string(), APPLICATION.to_string()),
            (identifier.to_string(), (*name).to_string()),
            ("SolutionStackName".to_string(), stack.clone()),
        ];
        params.extend(option_params(&borrowed(written)));
        created(&emulator, action, &as_params(&params)).await;

        let described = created(
            &emulator,
            "DescribeConfigurationSettings",
            &[("ApplicationName", APPLICATION), (identifier, name)],
        )
        .await;
        let reported = resolved_settings(&only_set(&described, case));
        assert_eq!(
            reported,
            expected_resolved(&stack, &defaults, written),
            "case {case:?}: the configuration set must be the published defaults with its own \
             settings over them, got {}",
            described.body
        );
        // The resolved set is a STRICT superset of what the request wrote: it
        // carries options nobody wrote at all.
        assert!(
            reported.len() > written.len(),
            "case {case:?}: expected more settings than the {} written, got {reported:?}",
            written.len()
        );
        // And one of them is the sibling of the option the reported case wrote,
        // in the same namespace, at the default the catalogue publishes for it
        // unless this row wrote it too.
        let published_max_size = defaults
            .get(&(ASG.to_string(), "MaxSize".to_string()))
            .expect("the published catalogue gives aws:autoscaling:asg/MaxSize a default");
        let expected_max_size = written
            .iter()
            .find(|(namespace, option_name, _, _)| namespace == ASG && option_name == "MaxSize")
            .map_or(published_max_size, |(_, _, value, _)| value);
        assert!(
            reported.contains(&setting(ASG, "MaxSize", expected_max_size, "")),
            "case {case:?}: expected aws:autoscaling:asg/MaxSize at {expected_max_size:?}, \
             got {reported:?}"
        );
    }
}

/// One row of the removal table: which options `OptionsToRemove` names, and the
/// settings the configuration is left holding of its own.
struct RemovalCase {
    case: &'static str,
    name: &'static str,
    removed: Vec<(&'static str, &'static str, &'static str)>,
    /// The settings the template still WRITES after the removal; everything else
    /// in the set it reports is a published default.
    remaining: Vec<Setting>,
}

#[tokio::test]
async fn removing_an_override_resolves_the_option_back_to_its_published_default() {
    let emulator = Emulator::new();
    created(
        &emulator,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    let stack = published_stack_of_category("Python").name();
    let defaults = published_defaults(&emulator, &stack).await;

    // Every row starts from this configuration: two options the catalogue
    // publishes defaults for, one it does not define at all, and one scoped to
    // a resource of its own.
    let written: Vec<Setting> = vec![
        setting(ASG, "MinSize", "6", ""),
        setting(ASG, "MaxSize", "9", ""),
        setting(APP_ENV, "STAGE", "qa", ""),
        setting(SCHEDULED, "MinSize", "4", "peak-hours"),
    ];

    let rows = vec![
        RemovalCase {
            // The reported case: the removed option does not vanish, it
            // resolves to the default the catalogue publishes for it.
            case: "remove an option the catalogue publishes a default for",
            name: "removes-min-size",
            removed: vec![(ASG, "MinSize", "")],
            remaining: vec![
                setting(ASG, "MaxSize", "9", ""),
                setting(APP_ENV, "STAGE", "qa", ""),
                setting(SCHEDULED, "MinSize", "4", "peak-hours"),
            ],
        },
        RemovalCase {
            // An option the catalogue does not define has no default to fall
            // back to, so removing it takes it out of the set.
            case: "remove an option the catalogue does not define",
            name: "removes-env-var",
            removed: vec![(APP_ENV, "STAGE", "")],
            remaining: vec![
                setting(ASG, "MinSize", "6", ""),
                setting(ASG, "MaxSize", "9", ""),
                setting(SCHEDULED, "MinSize", "4", "peak-hours"),
            ],
        },
        RemovalCase {
            // `ResourceName` is part of what identifies an option, so removing
            // the resource-scoped setting leaves the plain one alone.
            case: "remove the setting scoped to a resource",
            name: "removes-scheduled",
            removed: vec![(SCHEDULED, "MinSize", "peak-hours")],
            remaining: vec![
                setting(ASG, "MinSize", "6", ""),
                setting(ASG, "MaxSize", "9", ""),
                setting(APP_ENV, "STAGE", "qa", ""),
            ],
        },
        RemovalCase {
            case: "remove two options at once",
            name: "removes-both-sizes",
            removed: vec![(ASG, "MinSize", ""), (ASG, "MaxSize", "")],
            remaining: vec![
                setting(APP_ENV, "STAGE", "qa", ""),
                setting(SCHEDULED, "MinSize", "4", "peak-hours"),
            ],
        },
        RemovalCase {
            // An option the configuration never wrote is not there to remove:
            // naming it changes nothing, and that option keeps resolving to its
            // published default.
            case: "remove an option the configuration never wrote",
            name: "removes-absent",
            removed: vec![(ASG, "Cooldown", "")],
            remaining: vec![
                setting(ASG, "MinSize", "6", ""),
                setting(ASG, "MaxSize", "9", ""),
                setting(APP_ENV, "STAGE", "qa", ""),
                setting(SCHEDULED, "MinSize", "4", "peak-hours"),
            ],
        },
    ];

    for RemovalCase {
        case,
        name,
        removed,
        remaining,
    } in &rows
    {
        let mut params: Vec<(String, String)> = vec![
            ("ApplicationName".to_string(), APPLICATION.to_string()),
            ("TemplateName".to_string(), (*name).to_string()),
            ("SolutionStackName".to_string(), stack.clone()),
        ];
        params.extend(option_params(&borrowed(&written)));
        created(
            &emulator,
            "CreateConfigurationTemplate",
            &as_params(&params),
        )
        .await;

        let mut update: Vec<(String, String)> = vec![
            ("ApplicationName".to_string(), APPLICATION.to_string()),
            ("TemplateName".to_string(), (*name).to_string()),
        ];
        update.extend(remove_params(removed));
        let updated = created(
            &emulator,
            "UpdateConfigurationTemplate",
            &as_params(&update),
        )
        .await;
        let expected = expected_resolved(&stack, &defaults, remaining);
        assert_eq!(
            resolved_settings(&updated),
            expected,
            "case {case:?}: the update must answer with the resolved set, got {}",
            updated.body
        );

        // And the store answers the same way on a later read.
        let described = created(
            &emulator,
            "DescribeConfigurationSettings",
            &[("ApplicationName", APPLICATION), ("TemplateName", name)],
        )
        .await;
        let reported = resolved_settings(&only_set(&described, case));
        assert_eq!(
            reported, expected,
            "case {case:?}: a later read must report the same resolved set, got {}",
            described.body
        );
        // A removed option the catalogue publishes a default for is still
        // reported — at that default, not at the value the template wrote.
        for (namespace, option_name, resource_name) in removed {
            let Some(default_value) =
                defaults.get(&((*namespace).to_string(), (*option_name).to_string()))
            else {
                continue;
            };
            if !resource_name.is_empty() {
                continue;
            }
            assert!(
                reported.contains(&setting(namespace, option_name, default_value, "")),
                "case {case:?}: {namespace}/{option_name} must resolve to its published \
                 default {default_value:?}, got {reported:?}"
            );
        }
    }
}

/// The same fallback for a RUNNING environment: an `UpdateEnvironment` that
/// removes an option leaves that option in force at its published default once
/// the deployment the clock owed has completed.
#[tokio::test]
async fn removing_an_option_from_an_environment_leaves_it_at_its_published_default() {
    let case = "an environment's removed option";
    let emulator = Emulator::new();
    created(
        &emulator,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    let stack = published_stack_of_category("Python").name();
    let defaults = published_defaults(&emulator, &stack).await;
    let written: Vec<Setting> = vec![
        setting(ASG, "MinSize", "6", ""),
        setting(APP_ENV, "STAGE", "qa", ""),
    ];

    let mut params: Vec<(String, String)> = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("EnvironmentName".to_string(), "removal-env".to_string()),
        ("SolutionStackName".to_string(), stack.clone()),
    ];
    params.extend(option_params(&borrowed(&written)));
    created(&emulator, "CreateEnvironment", &as_params(&params)).await;
    // The launch the clock owed, waited for by the CONDITION it ends in: the
    // environment is `Ready` once the launch has completed.
    wait_for_status(&emulator, case, "removal-env", eb::state::STATUS_READY).await;

    let mut update: Vec<(String, String)> =
        vec![("EnvironmentName".to_string(), "removal-env".to_string())];
    update.extend(remove_params(&[(ASG, "MinSize", "")]));
    created(&emulator, "UpdateEnvironment", &as_params(&update)).await;
    // The update's own deployment: the configuration an update deploys is the
    // environment's draft until that deployment completes, which is again the
    // environment being back at `Ready`.
    wait_for_status(&emulator, case, "removal-env", eb::state::STATUS_READY).await;

    let described = created(
        &emulator,
        "DescribeConfigurationSettings",
        &[
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", "removal-env"),
        ],
    )
    .await;
    assert_eq!(
        resolved_settings(&only_set(&described, case)),
        expected_resolved(&stack, &defaults, &[setting(APP_ENV, "STAGE", "qa", "")]),
        "case {case:?}: the removed option must be back at its published default, got {}",
        described.body
    );
}

/// An option the published catalogue gives NO default for is in the set the
/// platform version DEFINES it in, in force at no value until the configuration
/// sets one: the read reports it either way, with no `Value` where nothing put
/// one in force and at the configuration's value where it did.
#[tokio::test]
async fn an_option_with_no_published_default_is_reported_at_no_value_until_the_set_sets_it() {
    let case = "an option with no published default";
    let emulator = Emulator::new();
    created(
        &emulator,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    let stack = published_stack_of_category("Go").name();
    let defaults = published_defaults(&emulator, &stack).await;

    // The option comes from the catalogue itself: one it publishes with no
    // `DefaultValue`, which is the caller's to set and nobody else's.
    let (namespace, option_name) =
        published_option_named(&emulator, &stack, case, |option| {
            option.default_value.is_none()
        })
        .await;

    for (name, written, expected_value) in [
        ("unset", Vec::new(), ""),
        (
            "set",
            vec![setting(&namespace, &option_name, "chosen", "")],
            "chosen",
        ),
    ] {
        let mut params: Vec<(String, String)> = vec![
            ("ApplicationName".to_string(), APPLICATION.to_string()),
            ("TemplateName".to_string(), name.to_string()),
            ("SolutionStackName".to_string(), stack.clone()),
        ];
        params.extend(option_params(&borrowed(&written)));
        created(
            &emulator,
            "CreateConfigurationTemplate",
            &as_params(&params),
        )
        .await;

        let described = created(
            &emulator,
            "DescribeConfigurationSettings",
            &[("ApplicationName", APPLICATION), ("TemplateName", name)],
        )
        .await;
        let reported = resolved_settings(&only_set(&described, case));
        assert_eq!(
            reported,
            expected_resolved(&stack, &defaults, &written),
            "case \"{case}, {name}\": unexpected resolved set, got {}",
            described.body
        );
        assert!(
            reported.contains(&setting(&namespace, &option_name, expected_value, "")),
            "case \"{case}, {name}\": expected {namespace}/{option_name} at \
             {expected_value:?}, got {reported:?}"
        );
    }
}

/// "Constraint: You can remove only `UserDefined` configuration options": an
/// option whose VALUE the service itself sets on the resources it provisions is
/// not the caller's to take out, so the removal is refused and the configuration
/// set is left as it stood.
#[tokio::test]
async fn removing_an_option_whose_value_the_service_sets_is_refused() {
    let case = "remove an option whose value the service sets";
    let emulator = Emulator::new();
    created(
        &emulator,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    let stack = published_stack_of_category("Python").name();
    let defaults = published_defaults(&emulator, &stack).await;
    let written: Vec<Setting> = vec![setting(ASG, "MinSize", "6", "")];

    // The Amazon SNS topic an environment's event notifications are published
    // to is CREATED by the service, so its ARN is a value the service sets
    // rather than one the caller supplies. Named through the service's own
    // constants, so the test and the catalogue cannot spell the option two
    // ways.
    let (namespace, option_name) = (
        eb::state::NAMESPACE_SNS_TOPICS.to_string(),
        eb::state::OPTION_NOTIFICATION_TOPIC_ARN.to_string(),
    );

    let mut params: Vec<(String, String)> = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("TemplateName".to_string(), "service-defined".to_string()),
        ("SolutionStackName".to_string(), stack.clone()),
    ];
    params.extend(option_params(&borrowed(&written)));
    created(
        &emulator,
        "CreateConfigurationTemplate",
        &as_params(&params),
    )
    .await;

    let mut update: Vec<(String, String)> = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("TemplateName".to_string(), "service-defined".to_string()),
    ];
    update.extend(remove_params(&[(
        namespace.as_str(),
        option_name.as_str(),
        "",
    )]));
    let refused = emulator
        .call("UpdateConfigurationTemplate", &as_params(&update))
        .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "InvalidParameterValue"),
        "case {case:?}: expected a 400 InvalidParameterValue, got {}",
        refused.body
    );

    let described = created(
        &emulator,
        "DescribeConfigurationSettings",
        &[
            ("ApplicationName", APPLICATION),
            ("TemplateName", "service-defined"),
        ],
    )
    .await;
    assert_eq!(
        resolved_settings(&only_set(&described, case)),
        expected_resolved(&stack, &defaults, &written),
        "case {case:?}: a refused update must leave the configuration set as it stood, got {}",
        described.body
    );
}
