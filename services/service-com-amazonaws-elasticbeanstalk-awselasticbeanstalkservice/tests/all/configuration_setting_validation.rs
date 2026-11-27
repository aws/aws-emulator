// SPDX-License-Identifier: CC0-1.0

//! `ValidateConfigurationSettings`, driven end to end over the service's awsQuery
//! HTTP surface, and the refusal the operations that WRITE option settings earn
//! for a value that evaluation calls an error.
//!
//! The rules under test:
//!
//! * every desired option setting is RESOLVED before it is evaluated: a
//!   specification whose `Namespace` no platform version defines names no
//!   configuration option, so it earns one error-severity `ValidationMessage`
//!   saying the setting is unknown — echoing the namespace and option name the
//!   caller sent, whatever value it carried — and an operation that WRITES it is
//!   refused;
//! * every desired option setting is evaluated against the restriction the
//!   PUBLISHED description of its own option carries on the platform version the
//!   named configuration set runs — the value set a selection draws from, the
//!   range a numeric option lies in, the length a string option is inside and
//!   the regular expression it matches — and one error-severity
//!   `ValidationMessage` per defective setting is reported, naming that
//!   setting's namespace, option and the value sent; a setting the option takes
//!   contributes none, which makes the advisories alone the answer to a valid
//!   selection;
//! * the configuration those values RESULT IN is reported with the advisories it
//!   carries as well — information the caller should take into account rather
//!   than a value to correct — each a warning-severity `ValidationMessage` scoped
//!   to the option the advice is about, reported after the errors, and a
//!   configuration that carries none is reported with none;
//! * a requirement one option states in terms of ANOTHER (`MinSize` must be "no
//!   greater than MaxSize") is a co-requirement between the two, resolved
//!   against the configuration the desired settings put IN FORCE: an inverted
//!   pair is reported on both halves, each naming the co-requirement the pair
//!   broke rather than a bound of its own option, and a request that raises only
//!   the minimum is reported against the maximum the configuration set already
//!   carries (`all/configuration_option_specification_resolution.rs` is the
//!   suite that spans that class);
//! * the configuration set the evaluation is made for is the one the request
//!   names — a stored template or a stored running environment, never both and
//!   never neither — and a request naming one this account does not hold is
//!   refused with this service's own code for that class;
//! * the evaluation REPORTS: it leaves the stored configuration exactly as it
//!   was, so a read after it answers what a read before it answered;
//! * an operation that WRITES option settings refuses a value that same
//!   evaluation calls an error, with `ConfigurationValidationException` and the
//!   sentence the service states a configuration it cannot accept with, and
//!   stores nothing — and is NOT refused for an advisory, which is information
//!   rather than a value the platform cannot honour.
//!
//! Each table row is a different point in those classes — a different
//! restriction kind, a different published bound, a different way of naming the
//! configuration set, a different writing operation — rather than a renamed copy
//! of the row before it. The published restrictions the rows are built from are
//! read from the catalogue the service publishes
//! (`eb::state::published_configuration_options`, what
//! `DescribeConfigurationOptions` reports), so no row states a bound this
//! service does not publish.
//!
//! The HTTP harness (`Emulator`, `Answer`) is the one `all/lifecycle.rs` owns in
//! this same test binary, and the option-setting encoder the one
//! `all/configuration_templates.rs` owns, so this suite keeps no second copy of
//! either and adds no dependency of its own.

use crate::configuration_templates::{as_params, created, option_params, published_stack};
use crate::harness::{Answer, Emulator};
use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application every configuration set below belongs to.
const APPLICATION: &str = "validate-app";
/// The stored configuration template the settings are validated against.
const TEMPLATE: &str = "validate-template";
/// The stored environment the settings are validated against.
const ENVIRONMENT: &str = "validate-env";

/// The Auto Scaling group namespace and its two size options, as the service's
/// own catalogue names them.
const ASG: &str = "aws:autoscaling:asg";
const MIN_SIZE: &str = "MinSize";
const MAX_SIZE: &str = "MaxSize";

/// The namespace whose option names the CALLER chooses (the environment
/// properties passed to the application), which the catalogue therefore
/// publishes no description for.
const APP_ENV: &str = "aws:elasticbeanstalk:application:environment";
/// A namespace whose options are scoped to a resource of their own, which this
/// service's catalogue likewise does not publish.
const SCHEDULED: &str = "aws:autoscaling:scheduledaction";

/// The launch-configuration namespace and the option in it that names the IAM
/// instance profile the instances a configuration launches are given.
const LAUNCH_CONFIGURATION: &str = "aws:autoscaling:launchconfiguration";
/// See [`LAUNCH_CONFIGURATION`].
const IAM_INSTANCE_PROFILE: &str = "IamInstanceProfile";

/// The sentence a configuration that associates no IAM instance profile with its
/// instances is advised with, word for word as the service states it: it names
/// nothing from the request, so it is written out here rather than built.
const NO_INSTANCE_PROFILE: &str = "The environment does not have an IAM instance profile \
                                   associated with it. To improve deployment speed please \
                                   associate an IAM instance profile with the environment.";

/// One `ValidationMessage` as the answer reports it, read member by member off
/// ONE element so an answer that mixes up which message belongs to which option
/// fails rather than matching independent flat lists.
///
/// `pub(crate)` along with [`reported_errors`], [`invalid_value`] and
/// [`published`] below: the `batch_size_validation_message` suite in this same
/// test target reads the very same `ValidationMessage`s off the wire and states
/// its rows against the same published catalogue, so it drives them through
/// these rather than keeping a second reader that could drift from this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReportedMessage {
    pub(crate) namespace: String,
    pub(crate) option_name: String,
    pub(crate) severity: String,
    pub(crate) message: String,
}

/// Every validation message in `answer`, in the order the document lists them.
fn reported_messages(answer: &Answer) -> Vec<ReportedMessage> {
    answer
        .elements_at("Messages/member")
        .iter()
        .map(|member| ReportedMessage {
            namespace: member.value("Namespace").unwrap_or_default().to_string(),
            option_name: member.value("OptionName").unwrap_or_default().to_string(),
            severity: member.value("Severity").unwrap_or_default().to_string(),
            message: member
                .value("Message")
                .unwrap_or_default()
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&amp;", "&"),
        })
        .collect()
}

/// The error-severity messages in `answer`: the findings about a value the
/// option does not take, read apart from the advisories the configuration
/// carries, so a row about one class is asserted on that class alone.
pub(crate) fn reported_errors(answer: &Answer) -> Vec<ReportedMessage> {
    reported_messages(answer)
        .into_iter()
        .filter(|message| message.severity == "error")
        .collect()
}

/// The advisory a configuration that associates no IAM instance profile carries,
/// as the answer must report it: the fixed sentence, at the advisory severity,
/// scoped to the option the advice is about.
fn instance_profile_advisory() -> ReportedMessage {
    ReportedMessage {
        namespace: LAUNCH_CONFIGURATION.to_string(),
        option_name: IAM_INSTANCE_PROFILE.to_string(),
        severity: "warning".to_string(),
        message: NO_INSTANCE_PROFILE.to_string(),
    }
}

/// The published description of `namespace` / `option_name` on the platform
/// version the fixtures below run, from the catalogue the service publishes —
/// the one `DescribeConfigurationOptions` reports, so a row cannot state a
/// restriction this service does not publish.
pub(crate) fn published(
    namespace: &str,
    option_name: &str,
) -> &'static eb::state::ConfigurationOption {
    let stack = published_stack(0);
    eb::state::published_configuration_options(Some(stack), true)
        .into_iter()
        .find(|option| option.namespace == namespace && option.name == option_name)
        .unwrap_or_else(|| panic!("the catalogue publishes {namespace} / {option_name}"))
}

/// The sentence a message about `value` for `namespace` / `option_name` opens
/// with: the value the request sent and the option it was sent for.
pub(crate) fn invalid_value(
    namespace: &str,
    option_name: &str,
    value: &str,
    requirement: &str,
) -> String {
    format!(
        "Invalid option value: '{value}' (Namespace: '{namespace}', OptionName: \
         '{option_name}'): {requirement}"
    )
}

/// An emulator holding the application, the template and the running environment
/// the rows validate against.
///
/// The template carries a `MaxSize` of its own, so a row can show the evaluation
/// reading the configuration set's STORED value rather than only the platform
/// version's default.
async fn emulator_with_configuration_sets() -> Emulator {
    let emulator = Emulator::new();
    created(&emulator, "CreateApplication", &[("ApplicationName", APPLICATION)]).await;
    let stack = published_stack(0).name();
    let mut template = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("TemplateName".to_string(), TEMPLATE.to_string()),
        ("SolutionStackName".to_string(), stack.clone()),
    ];
    template.extend(option_params(&[(ASG, MAX_SIZE, "9", "")]));
    created(
        &emulator,
        "CreateConfigurationTemplate",
        &as_params(&template),
    )
    .await;
    created(
        &emulator,
        "CreateEnvironment",
        &[
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", ENVIRONMENT),
            ("SolutionStackName", &stack),
        ],
    )
    .await;
    emulator
}

/// Validate `settings` against the stored template, requiring a 200: the
/// evaluation ANSWERS, and the messages are what it answers with.
async fn validated(emulator: &Emulator, settings: &[(&str, &str, &str, &str)]) -> Answer {
    let mut params = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("TemplateName".to_string(), TEMPLATE.to_string()),
    ];
    params.extend(option_params(settings));
    created(
        emulator,
        "ValidateConfigurationSettings",
        &as_params(&params),
    )
    .await
}

/// Look again at [`ENVIRONMENT`] until it reports `status`, or a deadline far
/// beyond the service's own transition span passes: the CONDITION is what is
/// waited for, never a fixed stretch of the clock.
async fn polled_until(emulator: &Emulator, case: &str, status: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let described = emulator
            .call(
                "DescribeEnvironments",
                &[
                    ("EnvironmentNames.member.1", ENVIRONMENT),
                    ("IncludeDeleted", "true"),
                ],
            )
            .await;
        if described.values_at("Environments/member/Status") == vec![status] {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the environment never reported itself {status}, last report {}",
            described.body
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// One row of the restriction table: the setting to evaluate and the
/// requirement the answer must report for it, where the option does not take the
/// value.
struct RestrictionCase {
    case: &'static str,
    namespace: &'static str,
    option_name: &'static str,
    value: String,
    /// The requirement the reported message must state, or `None` where the
    /// option takes the value and the answer must report nothing.
    requirement: Option<String>,
}

#[tokio::test]
async fn every_desired_value_is_evaluated_against_its_options_published_restriction() {
    let emulator = emulator_with_configuration_sets().await;

    // The bounds the rows are built from are the ones the catalogue publishes
    // for each option, so a row states no restriction this service does not
    // report through `DescribeConfigurationOptions`.
    let min_size = published(ASG, MIN_SIZE);
    let max_size = published(ASG, MAX_SIZE);
    let published_minimum = min_size
        .min_value
        .expect("the catalogue publishes a MinValue for the group's minimum size");
    let published_maximum = max_size
        .max_value
        .expect("the catalogue publishes a MaxValue for the group's maximum size");
    let system_type = published("aws:elasticbeanstalk:healthreporting:system", "SystemType");
    let allowed_system_types = system_type.value_options.join(", ");
    let vpc_id = published("aws:ec2:vpc", "VPCId");
    let max_length = vpc_id
        .max_length
        .expect("the catalogue publishes a MaxLength for the VPC id");
    let start_time = published("aws:elasticbeanstalk:managedactions", "PreferredStartTime");
    let (pattern, _) = start_time
        .regex
        .expect("the catalogue publishes a Regex for the maintenance window");

    // Rows differ along the restriction kind the value fails, and along where
    // the value sits relative to a published bound: at it, one past it, and
    // outside it in the other direction.
    let cases = vec![
        RestrictionCase {
            case: "inside the published range",
            namespace: ASG,
            option_name: MIN_SIZE,
            value: "2".to_string(),
            requirement: None,
        },
        RestrictionCase {
            case: "at the published maximum",
            namespace: ASG,
            option_name: MAX_SIZE,
            value: published_maximum.to_string(),
            requirement: None,
        },
        RestrictionCase {
            case: "one past the published maximum",
            namespace: ASG,
            option_name: MAX_SIZE,
            value: (i64::from(published_maximum) + 1).to_string(),
            requirement: Some(format!(
                "Value exceeds maximum allowed value: {published_maximum}"
            )),
        },
        RestrictionCase {
            case: "at the published minimum",
            namespace: ASG,
            option_name: MIN_SIZE,
            value: published_minimum.to_string(),
            requirement: None,
        },
        RestrictionCase {
            case: "one below the published minimum",
            namespace: ASG,
            option_name: MIN_SIZE,
            value: (i64::from(published_minimum) - 1).to_string(),
            requirement: Some(format!(
                "Value is below minimum allowed value: {published_minimum}"
            )),
        },
        RestrictionCase {
            case: "not a number where a range is published",
            namespace: ASG,
            option_name: MIN_SIZE,
            value: "two".to_string(),
            requirement: Some("Value is not a number".to_string()),
        },
        RestrictionCase {
            case: "inside the published value set",
            namespace: system_type.namespace,
            option_name: system_type.name,
            value: system_type
                .value_options
                .last()
                .expect("the catalogue publishes a value set for the health system")
                .to_string(),
            requirement: None,
        },
        RestrictionCase {
            case: "outside the published value set",
            namespace: system_type.namespace,
            option_name: system_type.name,
            value: "turbo".to_string(),
            requirement: Some(format!(
                "Value is not one of the allowed values: [{allowed_system_types}]"
            )),
        },
        RestrictionCase {
            case: "at the published length",
            namespace: vpc_id.namespace,
            option_name: vpc_id.name,
            value: "v".repeat(usize::try_from(max_length).expect("a published length is a count")),
            requirement: None,
        },
        RestrictionCase {
            case: "one past the published length",
            namespace: vpc_id.namespace,
            option_name: vpc_id.name,
            value: "v".repeat(
                usize::try_from(max_length).expect("a published length is a count") + 1,
            ),
            requirement: Some(format!("Value exceeds maximum allowed length: {max_length}")),
        },
        RestrictionCase {
            case: "matching the published pattern",
            namespace: start_time.namespace,
            option_name: start_time.name,
            value: "Tue:09:00".to_string(),
            requirement: None,
        },
        RestrictionCase {
            case: "outside the published pattern",
            namespace: start_time.namespace,
            option_name: start_time.name,
            value: "Funday:09:00".to_string(),
            requirement: Some(format!("Value does not match the required pattern: {pattern}")),
        },
        RestrictionCase {
            case: "an option the caller names, which the catalogue publishes no description for",
            namespace: APP_ENV,
            option_name: "STAGE",
            value: "qa".to_string(),
            requirement: None,
        },
        RestrictionCase {
            case: "an option scoped to a resource of its own",
            namespace: SCHEDULED,
            option_name: MIN_SIZE,
            value: "not-a-number".to_string(),
            requirement: None,
        },
    ];

    for case in &cases {
        let answer = validated(
            &emulator,
            &[(case.namespace, case.option_name, &case.value, "")],
        )
        .await;
        let reported = reported_errors(&answer);
        match case.requirement.as_deref() {
            None => assert!(
                reported.is_empty(),
                "case {:?}: expected no error-severity validation message, got {reported:?}",
                case.case
            ),
            Some(requirement) => assert_eq!(
                reported,
                vec![ReportedMessage {
                    namespace: case.namespace.to_string(),
                    option_name: case.option_name.to_string(),
                    severity: "error".to_string(),
                    message: invalid_value(
                        case.namespace,
                        case.option_name,
                        &case.value,
                        requirement
                    ),
                }],
                "case {:?}: expected one error naming the option and the requirement, got \
                 {reported:?}",
                case.case
            ),
        }
    }
}

#[tokio::test]
async fn one_message_is_reported_per_defective_setting() {
    let emulator = emulator_with_configuration_sets().await;

    // Three settings, two of them defective: the answer is about each setting
    // the request sent rather than about the first defect alone.
    let published_maximum = published(ASG, MAX_SIZE)
        .max_value
        .expect("the catalogue publishes a MaxValue for the group's maximum size");
    let above = (i64::from(published_maximum) + 1).to_string();
    let answer = validated(
        &emulator,
        &[
            (ASG, MAX_SIZE, &above, ""),
            (APP_ENV, "STAGE", "qa", ""),
            (
                "aws:elasticbeanstalk:healthreporting:system",
                "SystemType",
                "turbo",
                "",
            ),
        ],
    )
    .await;
    let reported: Vec<(String, String)> = reported_errors(&answer)
        .into_iter()
        .map(|message| (message.namespace, message.option_name))
        .collect();
    assert_eq!(
        reported,
        vec![
            (ASG.to_string(), MAX_SIZE.to_string()),
            (
                "aws:elasticbeanstalk:healthreporting:system".to_string(),
                "SystemType".to_string()
            ),
        ],
        "case \"two defective settings of three\": expected one message per defective setting \
         in request order, got {}",
        answer.body
    );
}

/// The two clauses the Auto Scaling group's size co-requirement is reported
/// with, one per side of the pair — word for word as the service states them.
const MIN_ABOVE_MAX: &str = "MinSize is greater than MaxSize";
/// See [`MIN_ABOVE_MAX`].
const MAX_BELOW_MIN: &str = "MaxSize is less than MinSize";

#[tokio::test]
async fn an_inverted_auto_scaling_group_pair_is_reported_on_both_halves() {
    let emulator = emulator_with_configuration_sets().await;

    // Both halves of the published range admit both values, so neither of them
    // is outside a bound of its own option: what the pair breaks is the
    // requirement BETWEEN the two options.
    let min_size = published(ASG, MIN_SIZE);
    let max_size = published(ASG, MAX_SIZE);
    for (option, value) in [(min_size, 4), (max_size, 2)] {
        assert!(
            option.min_value.is_none_or(|minimum| value >= minimum)
                && option.max_value.is_none_or(|maximum| value <= maximum),
            "case {:?}: expected {value} to be inside the published range {:?}..{:?}",
            option.name,
            option.min_value,
            option.max_value
        );
    }

    // Both halves sent, inverted: each one is reported against the
    // co-requirement the pair broke, read from its own side.
    let answer = validated(
        &emulator,
        &[(ASG, MIN_SIZE, "4", ""), (ASG, MAX_SIZE, "2", "")],
    )
    .await;
    let reported = reported_errors(&answer);
    assert_eq!(
        reported,
        vec![
            ReportedMessage {
                namespace: ASG.to_string(),
                option_name: MIN_SIZE.to_string(),
                severity: "error".to_string(),
                message: invalid_value(ASG, MIN_SIZE, "4", MIN_ABOVE_MAX),
            },
            ReportedMessage {
                namespace: ASG.to_string(),
                option_name: MAX_SIZE.to_string(),
                severity: "error".to_string(),
                message: invalid_value(ASG, MAX_SIZE, "2", MAX_BELOW_MIN),
            },
        ],
        "case \"an inverted pair\": expected an error on each half of the pair, each naming the \
         co-requirement the pair broke, got {reported:?}"
    );

    // Only the minimum sent, above the maximum the TEMPLATE carries (9): the
    // other half of the requirement comes from the configuration in force, so
    // the half the request sent is the one reported.
    let answer = validated(&emulator, &[(ASG, MIN_SIZE, "12", "")]).await;
    let reported = reported_errors(&answer);
    assert_eq!(
        reported,
        vec![ReportedMessage {
            namespace: ASG.to_string(),
            option_name: MIN_SIZE.to_string(),
            severity: "error".to_string(),
            message: invalid_value(ASG, MIN_SIZE, "12", MIN_ABOVE_MAX),
        }],
        "case \"above the stored maximum\": expected one message about the option the request \
         set, got {reported:?}"
    );

    // Below that stored maximum, the same option is accepted — the evaluation
    // reads the configuration set's own value, not only the platform default.
    let answer = validated(&emulator, &[(ASG, MIN_SIZE, "8", "")]).await;
    assert!(
        reported_errors(&answer).is_empty(),
        "case \"below the stored maximum\": expected no error, got {}",
        answer.body
    );
}

/// One row of the advisory table: the configuration set the evaluation is made
/// for, the settings the request sends for it, and whether the answer must carry
/// the instance-profile advisory.
struct AdvisoryCase {
    case: &'static str,
    /// The request members that name the configuration set.
    named: Vec<(String, String)>,
    settings: Vec<(&'static str, &'static str, &'static str, &'static str)>,
    advised: bool,
}

#[tokio::test]
async fn a_configuration_that_associates_no_instance_profile_is_advised() {
    let emulator = emulator_with_configuration_sets().await;

    // A second template that carries the profile ITSELF: the advisory is about
    // the configuration the desired settings RESULT IN, so a set that already
    // associates one is not advised for a request that says nothing about it.
    const WITH_PROFILE: &str = "profiled-template";
    let mut template = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("TemplateName".to_string(), WITH_PROFILE.to_string()),
        ("SolutionStackName".to_string(), published_stack(0).name()),
    ];
    template.extend(option_params(&[(
        LAUNCH_CONFIGURATION,
        IAM_INSTANCE_PROFILE,
        "my-instance-profile",
        "",
    )]));
    created(
        &emulator,
        "CreateConfigurationTemplate",
        &as_params(&template),
    )
    .await;

    let against = |member: &str, name: &str| -> Vec<(String, String)> {
        vec![
            ("ApplicationName".to_string(), APPLICATION.to_string()),
            (member.to_string(), name.to_string()),
        ]
    };

    // Rows differ along where the profile is associated (nowhere, by the
    // request, by the configuration set, unset by the request over a set that
    // carries it), along the value it is associated by, and along which kind of
    // configuration set the evaluation is made for.
    let cases = vec![
        AdvisoryCase {
            case: "a request that says nothing about the profile",
            named: against("TemplateName", TEMPLATE),
            settings: vec![(ASG, MIN_SIZE, "2", "")],
            advised: true,
        },
        AdvisoryCase {
            case: "a request that associates a profile",
            named: against("TemplateName", TEMPLATE),
            settings: vec![(LAUNCH_CONFIGURATION, IAM_INSTANCE_PROFILE, "eb-ec2-role", "")],
            advised: false,
        },
        AdvisoryCase {
            case: "a request that associates a differently named profile",
            named: against("TemplateName", TEMPLATE),
            settings: vec![(
                LAUNCH_CONFIGURATION,
                IAM_INSTANCE_PROFILE,
                "another-instance-profile",
                "",
            )],
            advised: false,
        },
        AdvisoryCase {
            case: "a request that sets the profile option to no value",
            named: against("TemplateName", TEMPLATE),
            settings: vec![(LAUNCH_CONFIGURATION, IAM_INSTANCE_PROFILE, "", "")],
            advised: true,
        },
        AdvisoryCase {
            case: "a request that sets the profile option to blanks",
            named: against("TemplateName", TEMPLATE),
            settings: vec![(LAUNCH_CONFIGURATION, IAM_INSTANCE_PROFILE, "   ", "")],
            advised: true,
        },
        AdvisoryCase {
            case: "a configuration set that carries the profile itself",
            named: against("TemplateName", WITH_PROFILE),
            settings: vec![(ASG, MIN_SIZE, "2", "")],
            advised: false,
        },
        AdvisoryCase {
            case: "a request that unsets the profile the configuration set carries",
            named: against("TemplateName", WITH_PROFILE),
            settings: vec![(LAUNCH_CONFIGURATION, IAM_INSTANCE_PROFILE, "", "")],
            advised: true,
        },
        AdvisoryCase {
            case: "a running environment that associates none",
            named: against("EnvironmentName", ENVIRONMENT),
            settings: vec![(ASG, MIN_SIZE, "2", "")],
            advised: true,
        },
    ];

    for case in &cases {
        let mut params = case.named.clone();
        params.extend(option_params(&case.settings));
        let answer = created(
            &emulator,
            "ValidateConfigurationSettings",
            &as_params(&params),
        )
        .await;
        let reported = reported_messages(&answer);
        let expected: Vec<ReportedMessage> = if case.advised {
            vec![instance_profile_advisory()]
        } else {
            Vec::new()
        };
        assert_eq!(
            reported, expected,
            "case {:?}: expected {} in the 200 body, got {reported:?}",
            case.case,
            if case.advised {
                "the instance-profile advisory"
            } else {
                "no message at all"
            }
        );
    }
}

#[tokio::test]
async fn the_advisories_are_reported_after_the_errors() {
    let emulator = emulator_with_configuration_sets().await;
    let published_maximum = published(ASG, MAX_SIZE)
        .max_value
        .expect("the catalogue publishes a MaxValue for the group's maximum size");
    let above = (i64::from(published_maximum) + 1).to_string();

    // One value the option does not take, and a configuration that associates no
    // profile: both severities are reported, in the 200 body, the errors first.
    let answer = validated(
        &emulator,
        &[
            (ASG, MAX_SIZE, &above, ""),
            (ASG, MIN_SIZE, "not-a-number", ""),
        ],
    )
    .await;
    assert_eq!(
        reported_messages(&answer),
        vec![
            ReportedMessage {
                namespace: ASG.to_string(),
                option_name: MAX_SIZE.to_string(),
                severity: "error".to_string(),
                message: invalid_value(
                    ASG,
                    MAX_SIZE,
                    &above,
                    &format!("Value exceeds maximum allowed value: {published_maximum}")
                ),
            },
            ReportedMessage {
                namespace: ASG.to_string(),
                option_name: MIN_SIZE.to_string(),
                severity: "error".to_string(),
                message: invalid_value(ASG, MIN_SIZE, "not-a-number", "Value is not a number"),
            },
            instance_profile_advisory(),
        ],
        "case \"two defective values and no instance profile\": expected the errors in request \
         order, then the advisory, got {}",
        answer.body
    );
}

#[tokio::test]
async fn a_write_is_not_refused_for_an_advisory() {
    let emulator = emulator_with_configuration_sets().await;
    let advised = validated(&emulator, &[(ASG, MIN_SIZE, "2", "")]).await;
    assert_eq!(
        reported_messages(&advised),
        vec![instance_profile_advisory()],
        "case \"before the write\": expected the advisory the write must not be refused for, \
         got {}",
        advised.body
    );

    // The same settings written through: an advisory is information the caller
    // should take into account, not a value the platform cannot honour, so the
    // write is performed and the configuration set carries it.
    let mut params = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("TemplateName".to_string(), TEMPLATE.to_string()),
    ];
    params.extend(option_params(&[(ASG, MIN_SIZE, "2", "")]));
    let written = emulator
        .call("UpdateConfigurationTemplate", &as_params(&params))
        .await;
    assert_eq!(
        written.status, 200,
        "case \"a write of an advised configuration\": expected the write to be performed, got \
         {} and body {}",
        written.status, written.body
    );
    let settings = created(
        &emulator,
        "DescribeConfigurationSettings",
        &[("ApplicationName", APPLICATION), ("TemplateName", TEMPLATE)],
    )
    .await;
    let stored_minimum: Vec<String> = settings
        .elements_at("OptionSettings/member")
        .iter()
        .filter(|member| {
            member.value("Namespace") == Some(ASG) && member.value("OptionName") == Some(MIN_SIZE)
        })
        .filter_map(|member| member.value("Value").map(str::to_string))
        .collect();
    assert_eq!(
        stored_minimum,
        vec!["2".to_string()],
        "case \"after the write\": expected the advised value stored, got {}",
        settings.body
    );
}

#[tokio::test]
async fn the_configuration_set_validated_against_is_the_one_the_request_names() {
    let emulator = emulator_with_configuration_sets().await;

    // The template carries `MaxSize` 9 and the environment carries none, so the
    // same desired minimum is valid for one configuration set and not for the
    // other: the evaluation is made for the set the request named.
    let mut against_environment = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("EnvironmentName".to_string(), ENVIRONMENT.to_string()),
    ];
    against_environment.extend(option_params(&[(ASG, MIN_SIZE, "8", "")]));
    let answer = created(
        &emulator,
        "ValidateConfigurationSettings",
        &as_params(&against_environment),
    )
    .await;
    let reported = reported_errors(&answer);
    assert_eq!(
        reported
            .iter()
            .map(|message| message.option_name.clone())
            .collect::<Vec<String>>(),
        vec![MIN_SIZE.to_string()],
        "case \"against the environment\": the environment's maximum is the platform default, \
         so the minimum above it must be reported, got {reported:?}"
    );

    let answer = validated(&emulator, &[(ASG, MIN_SIZE, "8", "")]).await;
    assert!(
        reported_errors(&answer).is_empty(),
        "case \"against the template\": the template's own maximum admits the minimum, got {}",
        answer.body
    );
}

#[tokio::test]
async fn the_evaluation_leaves_the_stored_configuration_as_it_was() {
    let emulator = emulator_with_configuration_sets().await;

    let settings_before = emulator
        .call(
            "DescribeConfigurationSettings",
            &[("ApplicationName", APPLICATION), ("TemplateName", TEMPLATE)],
        )
        .await;
    let before = settings_before.values_at("OptionSettings/member/Value");

    // A valid selection and a defective one, so neither answer can have written
    // anything.
    validated(&emulator, &[(ASG, MIN_SIZE, "3", "")]).await;
    validated(&emulator, &[(ASG, MAX_SIZE, "99999", "")]).await;

    let settings_after = emulator
        .call(
            "DescribeConfigurationSettings",
            &[("ApplicationName", APPLICATION), ("TemplateName", TEMPLATE)],
        )
        .await;
    assert_eq!(
        settings_after.values_at("OptionSettings/member/Value"),
        before,
        "case \"a read after the evaluation\": expected the stored configuration unchanged, got \
         {}",
        settings_after.body
    );

    // The same request answers the same way twice: the evaluation is a report,
    // so nothing it did changes what the next caller is told.
    let first = validated(&emulator, &[(ASG, MAX_SIZE, "99999", "")]).await;
    let second = validated(&emulator, &[(ASG, MAX_SIZE, "99999", "")]).await;
    assert_eq!(
        reported_messages(&first),
        reported_messages(&second),
        "case \"twice over\": expected the same answer, got {} and {}",
        first.body,
        second.body
    );
}

/// One refused request, and the code and message it must be refused with.
struct RefusalCase {
    case: &'static str,
    params: Vec<(String, String)>,
    code: &'static str,
    message: String,
}

#[tokio::test]
async fn a_request_that_names_no_single_configuration_set_is_refused() {
    let emulator = emulator_with_configuration_sets().await;
    let settings = option_params(&[(ASG, MIN_SIZE, "2", "")]);
    let with_settings = |named: &[(&str, &str)]| -> Vec<(String, String)> {
        let mut params: Vec<(String, String)> = named
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        params.extend(settings.clone());
        params
    };

    // Rows differ along WHICH half of the request cannot be served: both names,
    // neither name, and each of the three resources a name can fail to resolve.
    let cases = vec![
        RefusalCase {
            case: "both a template and an environment",
            params: with_settings(&[
                ("ApplicationName", APPLICATION),
                ("TemplateName", TEMPLATE),
                ("EnvironmentName", ENVIRONMENT),
            ]),
            code: "InvalidParameterCombination",
            message: "You cannot specify both TemplateName and EnvironmentName.".to_string(),
        },
        RefusalCase {
            case: "neither a template nor an environment",
            params: with_settings(&[("ApplicationName", APPLICATION)]),
            code: "MissingParameter",
            message: "Must specify either a TemplateName or an EnvironmentName.".to_string(),
        },
        RefusalCase {
            case: "an application this account does not hold",
            params: with_settings(&[
                ("ApplicationName", "no-such-app"),
                ("TemplateName", TEMPLATE),
            ]),
            code: "InvalidParameterValue",
            message: "No Application named 'no-such-app' found.".to_string(),
        },
        RefusalCase {
            case: "a template the application does not hold",
            params: with_settings(&[
                ("ApplicationName", APPLICATION),
                ("TemplateName", "no-such-template"),
            ]),
            code: "InvalidParameterValue",
            message: eb::state::missing_configuration_template_message(
                APPLICATION,
                "no-such-template",
            ),
        },
        RefusalCase {
            case: "an environment this account does not hold",
            params: with_settings(&[
                ("ApplicationName", APPLICATION),
                ("EnvironmentName", "no-such-env"),
            ]),
            code: "InvalidParameterValue",
            message: eb::state::missing_environment_message(None, Some("no-such-env")),
        },
    ];

    for case in &cases {
        let refused = emulator
            .call("ValidateConfigurationSettings", &as_params(&case.params))
            .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, case.code),
            "case {:?}: expected HTTP 400 {}, got {} and body {}",
            case.case,
            case.code,
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            case.message,
            "case {:?}: expected the refusal to state what cannot be served, got {}",
            case.case,
            refused.body
        );
    }
}

#[tokio::test]
async fn a_terminated_environment_runs_no_configuration_to_validate_against() {
    let emulator = emulator_with_configuration_sets().await;
    // A termination can only take down an environment whose launch has got past
    // its initial phase, which is what the clock owes it: waited for as a
    // CONDITION, never as a fixed time.
    polled_until(&emulator, "a launched environment", "Ready").await;
    created(
        &emulator,
        "TerminateEnvironment",
        &[("EnvironmentName", ENVIRONMENT)],
    )
    .await;
    // The termination the clock owes is materialised by the next call that
    // observes it, so the evaluation is asked once the environment reports
    // itself terminated.
    polled_until(&emulator, "a terminated environment", "Terminated").await;

    let mut params = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("EnvironmentName".to_string(), ENVIRONMENT.to_string()),
    ];
    params.extend(option_params(&[(ASG, MIN_SIZE, "2", "")]));
    let refused = emulator
        .call("ValidateConfigurationSettings", &as_params(&params))
        .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "InvalidParameterValue"),
        "case \"a terminated environment\": expected HTTP 400 InvalidParameterValue, got {} and \
         body {}",
        refused.status,
        refused.body
    );
}

/// One writing operation, and the request that writes the defective value
/// through it.
struct WriteCase {
    case: &'static str,
    action: &'static str,
    /// The request members beyond the option settings.
    named: Vec<(String, String)>,
}

#[tokio::test]
async fn a_write_of_a_value_the_platform_version_does_not_accept_is_refused() {
    let emulator = emulator_with_configuration_sets().await;
    let stack = published_stack(0).name();
    // An environment can only be updated once the launch the clock owes has
    // completed, so the row that writes through `UpdateEnvironment` waits for
    // the environment to report itself ready first.
    polled_until(&emulator, "a write to a running environment", "Ready").await;
    let published_maximum = published(ASG, MAX_SIZE)
        .max_value
        .expect("the catalogue publishes a MaxValue for the group's maximum size");
    let above = (i64::from(published_maximum) + 1).to_string();
    let expected = format!(
        "Configuration validation exception: {}",
        invalid_value(
            ASG,
            MAX_SIZE,
            &above,
            &format!("Value exceeds maximum allowed value: {published_maximum}")
        )
    );

    // Rows are the four operations that WRITE option settings: the rule is the
    // evaluation's, not any one handler's.
    let cases = vec![
        WriteCase {
            case: "CreateConfigurationTemplate",
            action: "CreateConfigurationTemplate",
            named: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), "refused-template".to_string()),
                ("SolutionStackName".to_string(), stack.clone()),
            ],
        },
        WriteCase {
            case: "UpdateConfigurationTemplate",
            action: "UpdateConfigurationTemplate",
            named: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), TEMPLATE.to_string()),
            ],
        },
        WriteCase {
            case: "CreateEnvironment",
            action: "CreateEnvironment",
            named: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("EnvironmentName".to_string(), "refused-env".to_string()),
                ("SolutionStackName".to_string(), stack.clone()),
            ],
        },
        WriteCase {
            case: "UpdateEnvironment",
            action: "UpdateEnvironment",
            named: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("EnvironmentName".to_string(), ENVIRONMENT.to_string()),
            ],
        },
    ];

    for case in &cases {
        let mut params = case.named.clone();
        params.extend(option_params(&[(ASG, MAX_SIZE, &above, "")]));
        let refused = emulator.call(case.action, &as_params(&params)).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "ConfigurationValidationException"),
            "case {:?}: expected HTTP 400 ConfigurationValidationException, got {} and body {}",
            case.case,
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            expected,
            "case {:?}: expected the refusal to carry the validation message, got {}",
            case.case,
            refused.body
        );
    }

    // A refused write stored nothing: the template it named does not exist and
    // the one it would have updated still carries what it carried.
    let applications = created(
        &emulator,
        "DescribeApplications",
        &[("ApplicationNames.member.1", APPLICATION)],
    )
    .await;
    assert_eq!(
        applications.values_at("ConfigurationTemplates/member"),
        vec![TEMPLATE],
        "case \"after the refusals\": expected the refused create to store nothing, got {}",
        applications.body
    );
    let settings = created(
        &emulator,
        "DescribeConfigurationSettings",
        &[("ApplicationName", APPLICATION), ("TemplateName", TEMPLATE)],
    )
    .await;
    let stored_maximum: Vec<String> = settings
        .elements_at("OptionSettings/member")
        .iter()
        .filter(|member| {
            member.value("Namespace") == Some(ASG) && member.value("OptionName") == Some(MAX_SIZE)
        })
        .filter_map(|member| member.value("Value").map(str::to_string))
        .collect();
    assert_eq!(
        stored_maximum,
        vec!["9".to_string()],
        "case \"after the refusals\": expected the template's stored maximum unchanged, got {}",
        settings.body
    );
}

// ---------------------------------------------------------------------------
// Resolving the specification, before evaluating the value in it
// ---------------------------------------------------------------------------

/// The sentence a specification the platform cannot resolve is reported with,
/// word for word, with the namespace and the option name the request sent in it.
fn unknown_specification(namespace: &str, option_name: &str) -> String {
    format!(
        "Invalid option specification (Namespace: '{namespace}', OptionName: \
         '{option_name}'): Unknown configuration setting."
    )
}

/// One row of the specification table: the setting to validate, and whether the
/// platform can resolve the namespace it names.
struct SpecificationCase {
    case: &'static str,
    namespace: &'static str,
    option_name: &'static str,
    value: &'static str,
    resolved: bool,
}

#[tokio::test]
async fn a_specification_naming_a_namespace_no_platform_defines_is_reported_as_unknown() {
    let emulator = emulator_with_configuration_sets().await;

    // Rows differ along HOW the namespace misses — a name of the service's own
    // shape that no platform defines, a near miss of one it does, a
    // resource-scoped prefix with nothing after it, and a name of no such shape
    // at all — and, for the ones the platform resolves, along WHY it resolves
    // them: described by the published catalogue, named by the caller, scoped to
    // a resource of its own, described by no option here, and a scoped prefix
    // followed by the resource it is for.
    let cases = [
        SpecificationCase {
            case: "a namespace no platform version defines",
            namespace: "aws:elasticbeanstalk:namespace-no-platform-defines",
            option_name: MIN_SIZE,
            value: "2",
            resolved: false,
        },
        SpecificationCase {
            case: "a near miss of a defined namespace",
            namespace: "aws:autoscaling:asgroup",
            option_name: MAX_SIZE,
            value: "4",
            resolved: false,
        },
        SpecificationCase {
            case: "a resource-scoped prefix with no resource after it",
            namespace: "aws:elbv2:listener:",
            option_name: "Protocol",
            value: "HTTPS",
            resolved: false,
        },
        SpecificationCase {
            case: "a name of no namespace shape at all",
            namespace: "my-own-settings",
            option_name: "Setting",
            value: "on",
            resolved: false,
        },
        SpecificationCase {
            case: "an unresolved specification whose value its option would also refuse",
            namespace: "aws:autoscaling:asg-v2",
            option_name: MIN_SIZE,
            value: "not-a-number",
            resolved: false,
        },
        SpecificationCase {
            case: "a namespace the published catalogue describes",
            namespace: ASG,
            option_name: MIN_SIZE,
            value: "2",
            resolved: true,
        },
        SpecificationCase {
            case: "a namespace whose option names the caller chooses",
            namespace: APP_ENV,
            option_name: "STAGE",
            value: "qa",
            resolved: true,
        },
        SpecificationCase {
            case: "a namespace whose options are scoped to a resource of their own",
            namespace: SCHEDULED,
            option_name: MIN_SIZE,
            value: "3",
            resolved: true,
        },
        SpecificationCase {
            case: "a namespace this service describes no option of",
            namespace: "aws:rds:dbinstance",
            option_name: "DBAllocatedStorage",
            value: "10",
            resolved: true,
        },
        SpecificationCase {
            case: "a resource-scoped prefix followed by its resource",
            namespace: "aws:elbv2:listener:443",
            option_name: "Protocol",
            value: "HTTPS",
            resolved: true,
        },
        SpecificationCase {
            case: "another resource-scoped prefix followed by its resource",
            namespace: "aws:elasticbeanstalk:environment:process:api",
            option_name: "HealthCheckPath",
            value: "/health",
            resolved: true,
        },
    ];

    for case in &cases {
        let answer = validated(
            &emulator,
            &[(case.namespace, case.option_name, case.value, "")],
        )
        .await;
        let reported = reported_errors(&answer);
        if case.resolved {
            assert!(
                reported.is_empty(),
                "case {:?}: expected no error-severity validation message, got {reported:?}",
                case.case
            );
            continue;
        }
        assert_eq!(
            reported,
            vec![ReportedMessage {
                namespace: case.namespace.to_string(),
                option_name: case.option_name.to_string(),
                severity: "error".to_string(),
                message: unknown_specification(case.namespace, case.option_name),
            }],
            "case {:?}: expected exactly one error-severity message attributed to the pair the \
             request sent, got {reported:?}",
            case.case
        );
        // The finding is reported in the 200 body ALONGSIDE the advisories the
        // configuration carries, not instead of them.
        assert!(
            reported_messages(&answer).contains(&instance_profile_advisory()),
            "case {:?}: expected the advisory to be reported beside the error, got {}",
            case.case,
            answer.body
        );
    }
}

#[tokio::test]
async fn a_write_of_a_specification_the_platform_cannot_resolve_is_refused() {
    let emulator = emulator_with_configuration_sets().await;
    let stack = published_stack(0).name();
    // An environment can only be updated once the launch the clock owes has
    // completed, so the row that writes through `UpdateEnvironment` waits for
    // the environment to report itself ready first.
    polled_until(&emulator, "a write to a running environment", "Ready").await;

    const UNKNOWN: &str = "aws:elasticbeanstalk:namespace-no-platform-defines";
    let expected = format!(
        "Configuration validation exception: {}",
        unknown_specification(UNKNOWN, MIN_SIZE)
    );

    // Rows are the four operations that WRITE option settings: the resolution is
    // the evaluation's, not any one handler's.
    let cases = vec![
        WriteCase {
            case: "CreateConfigurationTemplate",
            action: "CreateConfigurationTemplate",
            named: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), "unresolved-template".to_string()),
                ("SolutionStackName".to_string(), stack.clone()),
            ],
        },
        WriteCase {
            case: "UpdateConfigurationTemplate",
            action: "UpdateConfigurationTemplate",
            named: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("TemplateName".to_string(), TEMPLATE.to_string()),
            ],
        },
        WriteCase {
            case: "CreateEnvironment",
            action: "CreateEnvironment",
            named: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("EnvironmentName".to_string(), "unresolved-env".to_string()),
                ("SolutionStackName".to_string(), stack.clone()),
            ],
        },
        WriteCase {
            case: "UpdateEnvironment",
            action: "UpdateEnvironment",
            named: vec![
                ("ApplicationName".to_string(), APPLICATION.to_string()),
                ("EnvironmentName".to_string(), ENVIRONMENT.to_string()),
            ],
        },
    ];

    for case in &cases {
        let mut params = case.named.clone();
        params.extend(option_params(&[(UNKNOWN, MIN_SIZE, "2", "")]));
        let refused = emulator.call(case.action, &as_params(&params)).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "ConfigurationValidationException"),
            "case {:?}: expected HTTP 400 ConfigurationValidationException, got {} and body {}",
            case.case,
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            expected,
            "case {:?}: expected the refusal to carry the validation message, got {}",
            case.case,
            refused.body
        );
    }

    // Each refused write stored nothing: the template it would have created does
    // not exist, and the one it would have updated carries neither the
    // unresolved setting nor a changed value of its own.
    let applications = created(
        &emulator,
        "DescribeApplications",
        &[("ApplicationNames.member.1", APPLICATION)],
    )
    .await;
    assert_eq!(
        applications.values_at("ConfigurationTemplates/member"),
        vec![TEMPLATE],
        "case \"after the refusals\": expected the refused create to store nothing, got {}",
        applications.body
    );
    let settings = created(
        &emulator,
        "DescribeConfigurationSettings",
        &[("ApplicationName", APPLICATION), ("TemplateName", TEMPLATE)],
    )
    .await;
    assert!(
        !settings.values_at("OptionSettings/member/Namespace").contains(&UNKNOWN),
        "case \"after the refusals\": expected the unresolved namespace absent from the stored \
         configuration, got {}",
        settings.body
    );

    // The template's own legal settings stayed writable: the refusal was about
    // the specification the request sent, and left nothing behind.
    let mut legal = vec![
        ("ApplicationName".to_string(), APPLICATION.to_string()),
        ("TemplateName".to_string(), TEMPLATE.to_string()),
    ];
    legal.extend(option_params(&[(ASG, MAX_SIZE, "7", "")]));
    let written = emulator
        .call("UpdateConfigurationTemplate", &as_params(&legal))
        .await;
    assert_eq!(
        written.status, 200,
        "case \"a legal write after the refusals\": expected it to be performed, got {} and body \
         {}",
        written.status, written.body
    );
    let settings = created(
        &emulator,
        "DescribeConfigurationSettings",
        &[("ApplicationName", APPLICATION), ("TemplateName", TEMPLATE)],
    )
    .await;
    let stored_maximum: Vec<String> = settings
        .elements_at("OptionSettings/member")
        .iter()
        .filter(|member| {
            member.value("Namespace") == Some(ASG) && member.value("OptionName") == Some(MAX_SIZE)
        })
        .filter_map(|member| member.value("Value").map(str::to_string))
        .collect();
    assert_eq!(
        stored_maximum,
        vec!["7".to_string()],
        "case \"a legal write after the refusals\": expected the new maximum stored, got {}",
        settings.body
    );
}
