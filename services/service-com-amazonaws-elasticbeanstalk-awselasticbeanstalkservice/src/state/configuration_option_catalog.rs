// SPDX-License-Identifier: CC0-1.0

//! The service-owned configuration-option catalog: what a platform version
//! DEFINES, which `DescribeConfigurationOptions` reports and the environment
//! derivations resolve an option's value from.
//!
//! Split out of [`super`] so the option tables live beside the code that reads
//! them.

use super::*;

/// `ConfigurationOptionDescription$ChangeSeverity`: "There is no interruption
/// to the environment or application availability."
pub const CHANGE_SEVERITY_NO_INTERRUPTION: &str = "NoInterruption";
/// See [`CHANGE_SEVERITY_NO_INTERRUPTION`]. "The environment is entirely
/// restarted, all A resources are deleted and recreated, and the environment is
/// unavailable during the process."
pub const CHANGE_SEVERITY_RESTART_ENVIRONMENT: &str = "RestartEnvironment";
/// See [`CHANGE_SEVERITY_NO_INTERRUPTION`]. "The environment is available the
/// entire time. However, a short application outage occurs when the application
/// servers on the running Amazon EC2 instances are restarted."
pub const CHANGE_SEVERITY_RESTART_APPLICATION_SERVER: &str = "RestartApplicationServer";

/// `ConfigurationOptionValueType`: "List: Values for this option are multiple
/// selections from the possible values." Read from the modelled value set, so
/// this service cannot publish a type the model does not declare.
pub const OPTION_VALUE_TYPE_LIST: &str = CONFIGURATION_OPTION_VALUE_TYPE_VALUES[0];
/// See [`OPTION_VALUE_TYPE_LIST`]. "Scalar: Values for this option are a single
/// selection from the possible values, or an unformatted string, or numeric
/// value governed by the `MIN/MAX/Regex` constraints."
///
/// The member's documentation also mentions a `Boolean` and a `Json` type, but
/// the model's `ConfigurationOptionValueType` enum declares only `List` and
/// `Scalar` ([`CONFIGURATION_OPTION_VALUE_TYPE_VALUES`]), and the model is what
/// this service answers against: an option whose value is a boolean is a
/// `Scalar` whose [`BOOLEAN_VALUE_OPTIONS`] say which two values it takes, which
/// carries the same information inside the declared value set.
pub const OPTION_VALUE_TYPE_SCALAR: &str = CONFIGURATION_OPTION_VALUE_TYPE_VALUES[1];

/// The two values a boolean-valued configuration option takes, as its
/// `ValueOptions`. See [`OPTION_VALUE_TYPE_SCALAR`].
pub const BOOLEAN_VALUE_OPTIONS: &[&str] = &["false", "true"];

/// The `false` of that pair — the value a boolean option that is OFF carries,
/// read from the published set so the catalog and the handlers that read a
/// boolean option out of a configuration cannot spell it two ways.
pub const BOOLEAN_FALSE: &str = BOOLEAN_VALUE_OPTIONS[0];
/// The `true` of that pair. See [`BOOLEAN_FALSE`].
pub const BOOLEAN_TRUE: &str = BOOLEAN_VALUE_OPTIONS[1];

/// One configuration option a platform version defines — one
/// `ConfigurationOptionDescription` before it reaches the wire.
///
/// `default_value` is the option's documented default and `""` means the option
/// HAS no default (the member is then absent rather than an empty string);
/// `value_options` empty likewise means the option draws from no fixed value
/// set. The four restriction members are each present only where the option
/// carries that restriction, which is what makes the published description
/// typed: a numeric option carries a range, a string option a length or a
/// regular expression, and a selection carries its value set.
#[derive(Debug, Clone, Copy)]
pub struct ConfigurationOption {
    pub namespace: &'static str,
    pub name: &'static str,
    pub default_value: &'static str,
    /// One of [`CHANGE_SEVERITY_NO_INTERRUPTION`],
    /// [`CHANGE_SEVERITY_RESTART_ENVIRONMENT`] and
    /// [`CHANGE_SEVERITY_RESTART_APPLICATION_SERVER`].
    pub change_severity: &'static str,
    /// Whether the SERVICE owns this option's VALUE — one it sets on the
    /// environment it provisions (the topic it created, the queue it created)
    /// rather than one the caller supplies. It is what decides whether an
    /// `OptionsToRemove` may take the option out of a configuration set; it is
    /// NOT the published `UserDefined` flag, which answers whether the USER
    /// defined the option and so is false for every option this catalog
    /// publishes (see [`ConfigurationOption::to_description`]).
    pub service_owned: bool,
    /// One of [`OPTION_VALUE_TYPE_LIST`] and [`OPTION_VALUE_TYPE_SCALAR`].
    pub value_type: &'static str,
    pub value_options: &'static [&'static str],
    pub min_value: Option<i32>,
    pub max_value: Option<i32>,
    pub max_length: Option<i32>,
    /// `(Pattern, Label)` — "the regular expression pattern that a string
    /// configuration option value with this restriction must match" and "a
    /// unique name representing this regular expression".
    pub regex: Option<(&'static str, &'static str)>,
}

impl ConfigurationOption {
    /// A scalar option with no restriction beyond its type: the four
    /// `with_*` builders below add the ones an option carries, so a table entry
    /// states only what is true of THAT option.
    pub const fn new(
        namespace: &'static str,
        name: &'static str,
        change_severity: &'static str,
        default_value: &'static str,
    ) -> Self {
        Self {
            namespace,
            name,
            default_value,
            change_severity,
            service_owned: false,
            value_type: OPTION_VALUE_TYPE_SCALAR,
            value_options: &[],
            min_value: None,
            max_value: None,
            max_length: None,
            regex: None,
        }
    }

    /// "If specified, values for the configuration option are selected from
    /// this list."
    pub const fn with_values(mut self, value_options: &'static [&'static str]) -> Self {
        self.value_options = value_options;
        self
    }

    /// A boolean-valued option: a scalar whose value set is `true` | `false`.
    /// See [`OPTION_VALUE_TYPE_SCALAR`].
    pub const fn boolean(self) -> Self {
        self.with_values(BOOLEAN_VALUE_OPTIONS)
    }

    /// An option whose value is "multiple selections from the possible values".
    pub const fn multi_valued(mut self) -> Self {
        self.value_type = OPTION_VALUE_TYPE_LIST;
        self
    }

    /// "The configuration option must be a numeric value greater than this
    /// value" and "less than this value".
    pub const fn with_range(mut self, min_value: i32, max_value: i32) -> Self {
        self.min_value = Some(min_value);
        self.max_value = Some(max_value);
        self
    }

    /// "The configuration option must be a string value no longer than this
    /// value."
    pub const fn with_max_length(mut self, max_length: i32) -> Self {
        self.max_length = Some(max_length);
        self
    }

    /// "The configuration option must be a string value that satisfies this
    /// regular expression."
    pub const fn with_regex(mut self, pattern: &'static str, label: &'static str) -> Self {
        self.regex = Some((pattern, label));
        self
    }

    /// An option whose VALUE the SERVICE sets rather than the caller — see
    /// [`Self::service_owned`].
    pub const fn service_owned(mut self) -> Self {
        self.service_owned = true;
        self
    }

    /// The identity of an option: "a unique namespace identifying the option's
    /// associated Amazon Web Services resource" and "the name of the
    /// configuration option" together, which is also what an
    /// `OptionSpecification` names one by.
    pub fn key(&self) -> (&'static str, &'static str) {
        (self.namespace, self.name)
    }

    /// Whether `specification` names this option: every member the
    /// specification STATES has to match, so a specification carrying only a
    /// namespace selects that whole namespace — "if specified, restricts the
    /// descriptions to only the specified options".
    ///
    /// `ResourceName` is "a unique resource name for a time-based scaling
    /// configuration option": it names the resource an option is SET on, not an
    /// option the catalog DEFINES, so it never restricts which option is
    /// described — and a specification that states it ALONE names no option of
    /// the catalog at all, so it selects nothing rather than everything.
    pub fn is_named_by(&self, specification: &OptionSpecification) -> bool {
        let namespace = specification.namespace.as_deref();
        let option_name = specification.option_name.as_deref();
        if namespace.is_none() && option_name.is_none() {
            return false;
        }
        namespace.is_none_or(|namespace| namespace == self.namespace)
            && option_name.is_none_or(|option_name| option_name == self.name)
    }

    /// The modelled `ConfigurationOptionDescription` for this option.
    pub fn to_description(&self) -> ConfigurationOptionDescription {
        ConfigurationOptionDescription {
            namespace: Some(self.namespace.to_string()),
            name: Some(self.name.to_string()),
            default_value: stated(self.default_value),
            change_severity: Some(self.change_severity.to_string()),
            // "true: This configuration option was defined by the user …
            // false: This configuration was not defined by the user." Every
            // option this catalog publishes is one the PLATFORM VERSION
            // defines, so none of them was defined by the user; an option the
            // caller defines (an application environment variable, which the
            // caller names) is not in the catalog at all, which is why there is
            // no `true` here to read off a catalog entry.
            user_defined: Some(false),
            value_type: Some(self.value_type.to_string()),
            value_options: (!self.value_options.is_empty()).then(|| {
                self.value_options
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect()
            }),
            min_value: self.min_value,
            max_value: self.max_value,
            max_length: self.max_length,
            regex: self.regex.map(|(pattern, label)| OptionRestrictionRegex {
                pattern: Some(pattern.to_string()),
                label: Some(label.to_string()),
            }),
            // The option reference's own sentence about the option, from the
            // one table that holds them (see
            // [`super::configuration_option_descriptions`]); a unit test there
            // pins that every published option has one.
            description: option_description(self.namespace, self.name).map(str::to_string),
        }
    }
}

/// `aws:elb:healthcheck` / `Target`'s documented default: the check the classic
/// load balancer makes on an instance to decide whether it is healthy, stated as
/// the protocol and the port it is made on.
pub(super) const DEFAULT_HEALTH_CHECK_TARGET: &str = "TCP:80";

/// The value a load-balancer listener port carries when the balancer does NOT
/// listen on it — `aws:elb:loadbalancer` / `LoadBalancerHTTPSPort`'s documented
/// default, since an environment's balancer serves no HTTPS listener until a
/// certificate is configured for one.
pub(super) const LISTENER_PORT_OFF: &str = "OFF";

/// `aws:autoscaling:updatepolicy:rollingupdate` / `Timeout`'s documented default
/// — "the maximum amount of time to wait for all instances in a batch of
/// instances to pass health checks", as the ISO 8601 duration the option is
/// stated in.
pub(super) const DEFAULT_ROLLING_UPDATE_TIMEOUT: &str = "PT30M";

/// The namespace holding the options of the CloudWatch alarm an environment's
/// Auto Scaling group scales on, as the published option reference names it
/// ("General options for all environments").
pub(super) const NAMESPACE_AUTOSCALING_TRIGGER: &str = "aws:autoscaling:trigger";

/// The upper bound the reference states for each of the trigger's three timing
/// options ("1 to 600"): the period a metric is measured over and how long a
/// threshold must be breached before the group scales, both in minutes, and how
/// MANY of those periods are evaluated, which is a count rather than a span —
/// which is why the bound is named for neither unit.
pub(super) const MAX_PUBLISHED_TRIGGER_BOUND: i32 = 600;

/// The metrics the trigger can watch, as the reference lists them for
/// `MeasureName`.
pub(super) const TRIGGER_MEASURE_NAMES: &[&str] = &[
    "CPUUtilization",
    "DiskReadBytes",
    "DiskReadOps",
    "DiskWriteBytes",
    "DiskWriteOps",
    "HealthyHostCount",
    "Latency",
    "NetworkIn",
    TRIGGER_MEASURE_NETWORK_OUT,
    "RequestCount",
    "UnHealthyHostCount",
];

/// `MeasureName`'s documented default — the bytes an instance sends out, which
/// is also why the thresholds beside it are stated in [`TRIGGER_UNIT_BYTES`].
pub(super) const TRIGGER_MEASURE_NETWORK_OUT: &str = "NetworkOut";

/// The units a watched metric can be stated in, as the reference lists them for
/// `Unit`.
pub(super) const TRIGGER_UNITS: &[&str] = &[
    "Bits",
    "Bits/Second",
    TRIGGER_UNIT_BYTES,
    "Bytes/Second",
    "Count",
    "Count/Second",
    "None",
    "Percent",
    "Seconds",
];

/// `Unit`'s documented default. See [`TRIGGER_UNITS`].
pub(super) const TRIGGER_UNIT_BYTES: &str = "Bytes";

/// The CloudWatch Logs retention periods a log group can be kept for, in days,
/// which is the value set of every `RetentionInDays` option below.
pub(super) const LOG_RETENTION_IN_DAYS: &[&str] = &[
    "1", "3", "5", "7", "14", "30", "60", "90", "120", "150", "180", "365", "400", "545", "731",
    "1827", "3653",
];

/// The namespace holding the proxy option of the Linux platforms, and the
/// option in it: "ProxyServer: specifies which web server should be used to
/// proxy connections to the application".
pub(super) const NAMESPACE_PROXY: &str = "aws:elasticbeanstalk:environment:proxy";
/// See [`NAMESPACE_PROXY`].
pub(super) const OPTION_PROXY_SERVER: &str = "ProxyServer";

/// The namespace holding the options that say how an application version is
/// deployed to an environment's instances.
pub(super) const NAMESPACE_COMMAND: &str = "aws:elasticbeanstalk:command";
/// The option in that namespace giving the "percentage or fixed number of
/// Amazon EC2 instances in the Auto Scaling group on which to simultaneously
/// perform deployments".
pub(super) const OPTION_BATCH_SIZE: &str = "BatchSize";
/// The option in the same namespace that says which of those two
/// [`OPTION_BATCH_SIZE`] is — "whether BatchSize is a fixed number of instances
/// or a percentage of them" — so the published bound a batch size is held to is
/// the bound for the type this option puts in force.
pub(super) const OPTION_BATCH_SIZE_TYPE: &str = "BatchSizeType";

/// The batch size type of a batch size stated as a share of the group, and
/// [`OPTION_BATCH_SIZE_TYPE`]'s documented default.
pub(super) const BATCH_SIZE_TYPE_PERCENTAGE: &str = "Percentage";
/// The value set [`OPTION_BATCH_SIZE_TYPE`] publishes: a batch size is either a
/// count of instances or a share of them.
pub(super) const BATCH_SIZE_TYPES: &[&str] = &["Fixed", BATCH_SIZE_TYPE_PERCENTAGE];

/// The highest share of an environment's instances one deployment batch can
/// cover, which is the `MaxValue` the option reference publishes for
/// [`OPTION_BATCH_SIZE`] (a percentage, so a hundred of them), and its
/// documented default — "deploy to all of the instances at once".
pub(super) const MAX_PUBLISHED_BATCH_SIZE: i32 = 100;

/// [`OPTION_BATCH_SIZE`]'s documented default as the catalog publishes it: the
/// whole of the group in one batch, which for the default batch size type is
/// that maximum share of it — written out as the option value it is, beside the
/// bound it is the whole of.
pub(super) const DEFAULT_BATCH_SIZE_VALUE: &str = "100";

/// The configuration options EVERY platform version this service publishes
/// defines: the Auto Scaling group and launch configuration, the instances and
/// the VPC they run in, the deployment command, the environment itself, health
/// reporting, log streaming, managed updates, notifications and the load
/// balancer. The namespaces and the defaults are Elastic Beanstalk's published
/// configuration-option reference ("General options for all environments").
///
/// Two of the defaults are NOT written out here: `MinSize` is
/// [`DEFAULT_MIN_SIZE`] and `EnvironmentType` is
/// [`ENVIRONMENT_TYPE_LOAD_BALANCED`], the same constants the environment
/// resource derivation reads, so the catalog cannot publish a default the rest
/// of this service does not honour. The Auto Scaling sizes' `MaxValue` is
/// [`MAX_PUBLISHED_ASG_SIZE`], the bound the option reference states for them.
pub const GENERAL_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    // aws:autoscaling:asg — "the minimum number of instances you want in your
    // Auto Scaling group" and the maximum, plus where they may be launched.
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_ASG,
        "Availability Zones",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "Any",
    )
    .with_values(&["Any", "Any 1", "Any 2", "Any 3"]),
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_ASG,
        "Cooldown",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "360",
    ),
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_ASG,
        OPTION_MAX_SIZE,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "4",
    )
    .with_range(0, MAX_PUBLISHED_ASG_SIZE),
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_ASG,
        OPTION_MIN_SIZE,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        DEFAULT_MIN_SIZE_VALUE,
    )
    .with_range(0, MAX_PUBLISHED_ASG_SIZE),
    // aws:autoscaling:trigger — the CloudWatch alarm the group scales on:
    // which metric is watched, over what period, and the two thresholds it
    // scales out and in at. Every environment whose group can scale defines
    // them, so they are part of the configuration a read of the set reports.
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_TRIGGER,
        "BreachDuration",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "5",
    )
    .with_range(1, MAX_PUBLISHED_TRIGGER_BOUND),
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_TRIGGER,
        "EvaluationPeriods",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "1",
    )
    .with_range(1, MAX_PUBLISHED_TRIGGER_BOUND),
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_TRIGGER,
        "LowerBreachScaleIncrement",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "-1",
    ),
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_TRIGGER,
        "LowerThreshold",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "2000000",
    ),
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_TRIGGER,
        "MeasureName",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        TRIGGER_MEASURE_NETWORK_OUT,
    )
    .with_values(TRIGGER_MEASURE_NAMES),
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_TRIGGER,
        "Period",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "5",
    )
    .with_range(1, MAX_PUBLISHED_TRIGGER_BOUND),
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_TRIGGER,
        "Statistic",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "Average",
    )
    .with_values(&["Average", "Maximum", "Minimum", "Sum"]),
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_TRIGGER,
        "Unit",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        TRIGGER_UNIT_BYTES,
    )
    .with_values(TRIGGER_UNITS),
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_TRIGGER,
        "UpperBreachScaleIncrement",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "1",
    ),
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_TRIGGER,
        "UpperThreshold",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "6000000",
    ),
    // aws:autoscaling:launchconfiguration — the instances themselves.
    ConfigurationOption::new(
        "aws:autoscaling:launchconfiguration",
        "DisableIMDSv1",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:autoscaling:launchconfiguration",
        "EC2KeyName",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        NAMESPACE_LAUNCH_CONFIGURATION,
        OPTION_IAM_INSTANCE_PROFILE,
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        DEFAULT_INSTANCE_PROFILE,
    )
    .with_max_length(500),
    // The single-valued form of the instance type an environment runs, which
    // the environment derivations read beside the Auto Scaling group's list
    // (`aws:ec2:instances` / `InstanceTypes`) — see
    // [`environment_instance_type`]. The default is the type an environment
    // that names none runs ([`DEFAULT_INSTANCE_TYPE`]), read from that one
    // constant so the published default cannot differ from what this service
    // launches.
    ConfigurationOption::new(
        NAMESPACE_LAUNCH_CONFIGURATION,
        OPTION_INSTANCE_TYPE,
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        DEFAULT_INSTANCE_TYPE,
    ),
    ConfigurationOption::new(
        "aws:autoscaling:launchconfiguration",
        "MonitoringInterval",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "5 minute",
    )
    .with_values(&["1 minute", "5 minute"]),
    ConfigurationOption::new(
        "aws:autoscaling:launchconfiguration",
        "RootVolumeSize",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    )
    .with_range(8, 1024),
    ConfigurationOption::new(
        "aws:autoscaling:launchconfiguration",
        "RootVolumeType",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    )
    .with_values(&["gp2", "gp3", "io1", "standard"]),
    ConfigurationOption::new(
        "aws:autoscaling:launchconfiguration",
        "SecurityGroups",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    // aws:autoscaling:updatepolicy:rollingupdate — how a configuration change
    // is rolled out across the group.
    ConfigurationOption::new(
        "aws:autoscaling:updatepolicy:rollingupdate",
        "RollingUpdateEnabled",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:autoscaling:updatepolicy:rollingupdate",
        "RollingUpdateType",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "Time",
    )
    .with_values(&["Health", "Immutable", "Time"]),
    // aws:ec2:instances — the instance types the group may launch.
    ConfigurationOption::new(
        "aws:ec2:instances",
        "EnableSpot",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:ec2:instances",
        "InstanceTypes",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    // "The architectures of the EC2 instance types" — several selections from
    // one value set, which is what the modelled `List` value type is for.
    ConfigurationOption::new(
        "aws:ec2:instances",
        "SupportedArchitectures",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "x86_64",
    )
    .multi_valued()
    .with_values(&["arm64", "x86_64"]),
    // aws:ec2:vpc — where the environment runs.
    ConfigurationOption::new(
        "aws:ec2:vpc",
        "AssociatePublicIpAddress",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:ec2:vpc",
        "ELBScheme",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "public",
    )
    .with_values(&["internal", "public"]),
    ConfigurationOption::new(
        "aws:ec2:vpc",
        "ELBSubnets",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    ConfigurationOption::new(
        "aws:ec2:vpc",
        "Subnets",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    ConfigurationOption::new("aws:ec2:vpc", "VPCId", CHANGE_SEVERITY_RESTART_ENVIRONMENT, "")
        .with_max_length(255),
    // aws:elasticbeanstalk:application — the application's health check path.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:application",
        "Application Healthcheck URL",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    )
    .with_max_length(255),
    // aws:elasticbeanstalk:cloudwatch:logs — instance log streaming.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:cloudwatch:logs",
        "DeleteOnTerminate",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:cloudwatch:logs",
        "RetentionInDays",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "7",
    )
    .with_values(LOG_RETENTION_IN_DAYS),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:cloudwatch:logs",
        "StreamLogs",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    // aws:elasticbeanstalk:cloudwatch:logs:health — environment health
    // streaming, a second log group with its own retention.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:cloudwatch:logs:health",
        "DeleteOnTerminate",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:cloudwatch:logs:health",
        "HealthStreamingEnabled",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:cloudwatch:logs:health",
        "RetentionInDays",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "7",
    )
    .with_values(LOG_RETENTION_IN_DAYS),
    // aws:elasticbeanstalk:command — how an application version is deployed.
    ConfigurationOption::new(
        NAMESPACE_COMMAND,
        OPTION_BATCH_SIZE,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        DEFAULT_BATCH_SIZE_VALUE,
    )
    .with_range(1, MAX_PUBLISHED_BATCH_SIZE),
    ConfigurationOption::new(
        NAMESPACE_COMMAND,
        OPTION_BATCH_SIZE_TYPE,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        BATCH_SIZE_TYPE_PERCENTAGE,
    )
    .with_values(BATCH_SIZE_TYPES),
    ConfigurationOption::new(
        NAMESPACE_COMMAND,
        "DeploymentPolicy",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "AllAtOnce",
    )
    .with_values(&[
        "AllAtOnce",
        "Immutable",
        "Rolling",
        "RollingWithAdditionalBatch",
        "TrafficSplitting",
    ]),
    ConfigurationOption::new(
        NAMESPACE_COMMAND,
        "IgnoreHealthCheck",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        NAMESPACE_COMMAND,
        "Timeout",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "600",
    )
    .with_range(1, 3600),
    // aws:elasticbeanstalk:environment — the environment itself.
    ConfigurationOption::new(
        NAMESPACE_ENVIRONMENT,
        OPTION_ENVIRONMENT_TYPE,
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        ENVIRONMENT_TYPE_LOAD_BALANCED,
    )
    .with_values(&[ENVIRONMENT_TYPE_LOAD_BALANCED, ENVIRONMENT_TYPE_SINGLE_INSTANCE]),
    ConfigurationOption::new(
        NAMESPACE_ENVIRONMENT,
        "LoadBalancerType",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "classic",
    )
    .with_values(&["application", "classic", "network"]),
    ConfigurationOption::new(
        NAMESPACE_ENVIRONMENT,
        "ServiceRole",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "aws-elasticbeanstalk-service-role",
    )
    .with_max_length(500),
    // aws:elasticbeanstalk:healthreporting:system — basic or enhanced health.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:healthreporting:system",
        "HealthCheckSuccessThreshold",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "Ok",
    )
    .with_values(&["Degraded", "Ok", "Severe", "Warning"]),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:healthreporting:system",
        "SystemType",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "basic",
    )
    .with_values(&["basic", "enhanced"]),
    // aws:elasticbeanstalk:hostmanager — instance log publication to Amazon S3.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:hostmanager",
        "LogPublicationControl",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    // aws:elasticbeanstalk:managedactions — managed platform updates, the
    // window they run in and the role that runs them.
    ConfigurationOption::new(
        NAMESPACE_MANAGED_ACTIONS,
        OPTION_MANAGED_ACTIONS_ENABLED,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        BOOLEAN_FALSE,
    )
    .boolean(),
    // "PreferredStartTime: configure a maintenance window for managed actions
    // in UTC", stated as `day:hour:minute` (`Tue:09:00`) — a string option whose
    // form IS its restriction, so the catalog publishes it as one.
    ConfigurationOption::new(
        NAMESPACE_MANAGED_ACTIONS,
        OPTION_PREFERRED_START_TIME,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    )
    .with_regex(PREFERRED_START_TIME_PATTERN, "DayOfWeekAndTime"),
    ConfigurationOption::new(
        NAMESPACE_MANAGED_ACTIONS,
        "ServiceRoleForManagedUpdates",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    )
    .with_max_length(500),
    ConfigurationOption::new(
        NAMESPACE_MANAGED_ACTIONS_PLATFORM_UPDATE,
        OPTION_INSTANCE_REFRESH_ENABLED,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        BOOLEAN_FALSE,
    )
    .boolean(),
    ConfigurationOption::new(
        NAMESPACE_MANAGED_ACTIONS_PLATFORM_UPDATE,
        OPTION_UPDATE_LEVEL,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    )
    .with_values(&[UPDATE_LEVEL_MINOR, UPDATE_LEVEL_PATCH]),
    // aws:elasticbeanstalk:monitoring — what the service does with an instance
    // enhanced health reports as failing.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:monitoring",
        "Automatically Terminate Unhealthy Instances",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "true",
    )
    .boolean(),
    // aws:elasticbeanstalk:sns:topics — environment event notifications, which
    // this service really publishes (see [`super::event_notifications`]); the
    // namespace and the option names are that module's, so the catalog and the
    // delivery cannot disagree about what they are called.
    ConfigurationOption::new(
        NAMESPACE_SNS_TOPICS,
        OPTION_NOTIFICATION_ENDPOINT,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        NAMESPACE_SNS_TOPICS,
        OPTION_NOTIFICATION_PROTOCOL,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "email",
    )
    .with_values(&["email", "email-json", "http", "https", "sqs"]),
    // The topic the service creates for those notifications: it reports the ARN
    // it created, so this one is not a value a caller sets.
    ConfigurationOption::new(
        NAMESPACE_SNS_TOPICS,
        OPTION_NOTIFICATION_TOPIC_ARN,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    )
    .service_owned(),
    ConfigurationOption::new(
        NAMESPACE_SNS_TOPICS,
        OPTION_NOTIFICATION_TOPIC_NAME,
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    )
    .with_max_length(255),
    // aws:elasticbeanstalk:xray — the X-Ray daemon.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:xray",
        "XRayEnabled",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "false",
    )
    .boolean(),
    // aws:elb:healthcheck / aws:elb:loadbalancer — the classic load balancer,
    // which is what `LoadBalancerType`'s default above provisions.
    ConfigurationOption::new(
        "aws:elb:healthcheck",
        "HealthyThreshold",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "3",
    )
    .with_range(2, 10),
    ConfigurationOption::new(
        "aws:elb:healthcheck",
        "Interval",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "10",
    )
    .with_range(5, 300),
    ConfigurationOption::new(
        "aws:elb:healthcheck",
        "Timeout",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "5",
    )
    .with_range(2, 60),
    ConfigurationOption::new(
        "aws:elb:healthcheck",
        "UnhealthyThreshold",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "5",
    )
    .with_range(2, 10),
    ConfigurationOption::new(
        "aws:elb:loadbalancer",
        "CrossZone",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "true",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:elb:loadbalancer",
        "SecurityGroups",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    ),
    // The REST of what the option reference defines in the namespaces above.
    //
    // A namespace the catalog describes any option of STATES that namespace's
    // option names, so a name it leaves out is reported as naming no
    // configuration setting
    // ([`super::configuration_setting_validation::configuration_validation_messages`]).
    // These are therefore not optional entries: every option the published
    // reference lists for a described namespace has to be here, or a
    // configuration the real service accepts is refused. They are the options
    // this service honours no BEHAVIOUR for, so each publishes its name, its
    // namespace and its change severity and no default value — the option HAS no
    // value here until a caller sets one — and no restriction this service
    // cannot ground, since a restriction published without one would refuse
    // values the platform takes.
    ConfigurationOption::new(
        NAMESPACE_AUTOSCALING_ASG,
        "Custom Availability Zones",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    ConfigurationOption::new(
        NAMESPACE_LAUNCH_CONFIGURATION,
        "BlockDeviceMappings",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    ConfigurationOption::new(
        NAMESPACE_LAUNCH_CONFIGURATION,
        "DisableDefaultEC2SecurityGroup",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        BOOLEAN_FALSE,
    )
    .boolean(),
    ConfigurationOption::new(
        NAMESPACE_LAUNCH_CONFIGURATION,
        "ImageId",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    ConfigurationOption::new(
        NAMESPACE_LAUNCH_CONFIGURATION,
        "RootVolumeIOPS",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    ConfigurationOption::new(
        NAMESPACE_LAUNCH_CONFIGURATION,
        "RootVolumeThroughput",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    ConfigurationOption::new(
        NAMESPACE_LAUNCH_CONFIGURATION,
        "SSHSourceRestriction",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    ConfigurationOption::new(
        "aws:autoscaling:updatepolicy:rollingupdate",
        "MaxBatchSize",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    ),
    ConfigurationOption::new(
        "aws:autoscaling:updatepolicy:rollingupdate",
        "MinInstancesInService",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    ),
    ConfigurationOption::new(
        "aws:autoscaling:updatepolicy:rollingupdate",
        "PauseTime",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    ),
    ConfigurationOption::new(
        "aws:autoscaling:updatepolicy:rollingupdate",
        "Timeout",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        DEFAULT_ROLLING_UPDATE_TIMEOUT,
    ),
    ConfigurationOption::new(
        "aws:ec2:instances",
        "SpotFleetOnDemandBase",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    ConfigurationOption::new(
        "aws:ec2:instances",
        "SpotFleetOnDemandAboveBasePercentage",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    ConfigurationOption::new(
        "aws:ec2:instances",
        "SpotMaxPrice",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    ConfigurationOption::new(
        "aws:ec2:vpc",
        "DBSubnets",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    ),
    ConfigurationOption::new(
        NAMESPACE_ENVIRONMENT,
        "ExternalExtensionsS3Bucket",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    ),
    ConfigurationOption::new(
        NAMESPACE_ENVIRONMENT,
        "ExternalExtensionsS3Key",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    ),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:healthreporting:system",
        "ConfigDocument",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    ),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:healthreporting:system",
        "EnhancedHealthAuthEnabled",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    ),
    // The classic load balancer's own listeners and health check, whose
    // defaults the option reference states: the check the balancer makes on an
    // instance, and the two ports it listens on with the protocol each speaks.
    ConfigurationOption::new(
        "aws:elb:healthcheck",
        "Target",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        DEFAULT_HEALTH_CHECK_TARGET,
    ),
    ConfigurationOption::new(
        "aws:elb:loadbalancer",
        "LoadBalancerHTTPPort",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "80",
    ),
    ConfigurationOption::new(
        "aws:elb:loadbalancer",
        "LoadBalancerHTTPSPort",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        LISTENER_PORT_OFF,
    ),
    ConfigurationOption::new(
        "aws:elb:loadbalancer",
        "LoadBalancerPortProtocol",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "HTTP",
    ),
    ConfigurationOption::new(
        "aws:elb:loadbalancer",
        "LoadBalancerSSLPortProtocol",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "HTTPS",
    ),
    ConfigurationOption::new(
        "aws:elb:loadbalancer",
        "ManagedSecurityGroup",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "",
    ),
];

/// The upper bound the option reference states for both Auto Scaling group
/// sizes: "Maximum number of instances you want running. Must be a positive
/// integer that is less than 10000", and `MinSize` "no greater than MaxSize".
/// It is the `MaxValue` the published description of each carries.
///
/// It is NOT [`MAX_ENVIRONMENT_INSTANCES`], the number of instances one
/// environment's stored inventory holds here: that bound is this emulator's own
/// (a larger `MinSize` is held down to it rather than materialised), so
/// publishing it as the option's restriction would tell a caller the service
/// refuses a group size the real one accepts.
pub const MAX_PUBLISHED_ASG_SIZE: i32 = 10000;

/// `aws:autoscaling:asg` / `MinSize`'s default as the catalog publishes it — the
/// option value [`DEFAULT_MIN_SIZE`] is the count of, checked against it below
/// so the two cannot drift apart.
pub(super) const DEFAULT_MIN_SIZE_VALUE: &str = "1";
const _: () = assert!(DEFAULT_MIN_SIZE == 1, "MinSize's published default is DEFAULT_MIN_SIZE");

/// The options the Amazon SQS daemon of a WORKER environment defines
/// (`aws:elasticbeanstalk:sqsd`): the queue it reads and how it hands each
/// message to the application.
///
/// A platform version defines these only where its branch supports the worker
/// tier — the same rule [`managed_supported_addons`] applies to the
/// [`ADDON_WORKER_DAEMON_SQSD`] addition, read from the branch's
/// `supported_tiers` so the two cannot disagree about which platforms run the
/// daemon.
pub const WORKER_TIER_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "ErrorVisibilityTimeout",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "2",
    )
    .with_range(0, 43200),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "HttpConnections",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "50",
    )
    .with_range(1, 100),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "HttpPath",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "/",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "InactivityTimeout",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "299",
    )
    .with_range(1, 36000),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "MaxRetries",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "10",
    )
    .with_range(0, 100),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "MimeType",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "application/json",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "RetentionPeriod",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "345600",
    )
    .with_range(60, 1209600),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "VisibilityTimeout",
        CHANGE_SEVERITY_NO_INTERRUPTION,
        "300",
    )
    .with_range(0, 43200),
    // "WorkerQueueURL: the URL of the queue … If not specified, Elastic
    // Beanstalk creates the queue" — which is what this service's own
    // [`EnvironmentResources`] provisioning does, so the value is the
    // service's rather than the caller's.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:sqsd",
        "WorkerQueueURL",
        CHANGE_SEVERITY_RESTART_ENVIRONMENT,
        "",
    )
    .service_owned(),
];

/// The web server the Linux platforms proxy through, where the platform offers
/// a choice of two ("ProxyServer: nginx or apache").
pub(super) const PROXY_NGINX_OR_APACHE: ConfigurationOption = ConfigurationOption::new(
    NAMESPACE_PROXY,
    OPTION_PROXY_SERVER,
    CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
    "nginx",
)
.with_values(&["apache", "nginx"]);

/// The same option on a platform that proxies through nginx or not at all.
pub(super) const PROXY_NGINX_OR_NONE: ConfigurationOption = ConfigurationOption::new(
    NAMESPACE_PROXY,
    OPTION_PROXY_SERVER,
    CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
    "nginx",
)
.with_values(&["nginx", "none"]);

/// The options a Python platform version defines beyond the general catalog.
pub(super) const PYTHON_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    PROXY_NGINX_OR_APACHE,
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:python",
        "NumProcesses",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "1",
    )
    .with_range(1, 1000),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:python",
        "NumThreads",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "15",
    )
    .with_range(1, 1000),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:python",
        "WSGIPath",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "application",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:python:staticfiles",
        "/static",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "static",
    )
    .with_max_length(255),
];

/// The options a PHP platform version defines beyond the general catalog: the
/// `php.ini` directives the platform sets.
pub(super) const PHP_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    PROXY_NGINX_OR_APACHE,
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:php:phpini",
        "allow_url_fopen",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "On",
    )
    .with_values(&["Off", "On"]),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:php:phpini",
        "composer_options",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "",
    )
    .with_max_length(200),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:php:phpini",
        "display_errors",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "Off",
    )
    .with_values(&["Off", "On"]),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:php:phpini",
        "document_root",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "",
    )
    .with_max_length(255),
    // "max_execution_time" is a count of seconds; the directive's own upper
    // bound is far outside `OptionRestrictionMaxValue`'s integer range, so the
    // catalog publishes the default and no range rather than a bound the model
    // cannot carry.
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:php:phpini",
        "max_execution_time",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "60",
    ),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:php:phpini",
        "memory_limit",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "256M",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:php:phpini",
        "zlib.output_compression",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "Off",
    )
    .with_values(&["Off", "On"]),
];

/// The options a Ruby platform version defines beyond the general catalog.
pub(super) const RUBY_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    PROXY_NGINX_OR_APACHE,
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:ruby",
        "RackEnv",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "production",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:ruby",
        "RailsEnv",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "production",
    )
    .with_max_length(255),
];

/// The options a Node.js platform version defines beyond the general catalog.
pub(super) const NODEJS_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    PROXY_NGINX_OR_NONE,
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:nodejs",
        "NodeCommand",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "",
    )
    .with_max_length(255),
];

/// The options a Tomcat platform version defines beyond the general catalog:
/// the JVM the application server runs in.
pub(super) const TOMCAT_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    PROXY_NGINX_OR_APACHE,
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:tomcat:jvmoptions",
        "JVM Options",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:tomcat:jvmoptions",
        "Xms",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "256m",
    )
    .with_max_length(255),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:tomcat:jvmoptions",
        "Xmx",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "256m",
    )
    .with_max_length(255),
];

/// The options a .NET-on-Windows platform version defines beyond the general
/// catalog: the IIS application pool the application runs in. A Windows
/// platform proxies through IIS itself, so it defines no `ProxyServer` option.
pub(super) const DOTNET_WINDOWS_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:dotnet:apppool",
        "Enable 32-bit Applications",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "false",
    )
    .boolean(),
    ConfigurationOption::new(
        "aws:elasticbeanstalk:container:dotnet:apppool",
        "Target Runtime",
        CHANGE_SEVERITY_RESTART_APPLICATION_SERVER,
        "4.0",
    )
    .with_values(&["2.0", "4.0"]),
];

/// The options a platform version defines because of the RUNTIME it runs,
/// by the [`SolutionStack::platform_category`] the catalog publishes it under.
///
/// This is the one place a platform-specific namespace is stated: the
/// `aws:elasticbeanstalk:container:<runtime>` namespaces exist only on the
/// platform whose application server they configure, and the proxy option's
/// value set follows the platform too (the Python, PHP and Ruby platforms
/// proxy through nginx or apache; the rest through nginx or nothing). A
/// category not listed here — Go and .NET on Linux run the application
/// directly behind the proxy, Docker runs whatever the image holds — defines
/// the proxy option alone.
pub(super) const PLATFORM_CATEGORY_CONFIGURATION_OPTIONS: &[(&str, &[ConfigurationOption])] = &[
    (".NET", DOTNET_WINDOWS_CONFIGURATION_OPTIONS),
    ("Node.js", NODEJS_CONFIGURATION_OPTIONS),
    ("PHP", PHP_CONFIGURATION_OPTIONS),
    ("Python", PYTHON_CONFIGURATION_OPTIONS),
    ("Ruby", RUBY_CONFIGURATION_OPTIONS),
    ("Tomcat", TOMCAT_CONFIGURATION_OPTIONS),
];

/// The options every OTHER Linux platform category defines: the proxy choice
/// its web tier offers. See [`PLATFORM_CATEGORY_CONFIGURATION_OPTIONS`].
pub(super) const LINUX_CONFIGURATION_OPTIONS: &[ConfigurationOption] = &[PROXY_NGINX_OR_NONE];

/// The namespace holding the parameters Elastic Beanstalk fills into the
/// CloudFormation template it builds for an environment. It is not a namespace a
/// platform version publishes a configurable option description of (it is listed
/// in
/// [`super::configuration_option_namespaces::UNDESCRIBED_CONFIGURATION_NAMESPACES`]),
/// and it carries options a configuration SET reports — see
/// [`PLATFORM_SUPPLIED_OPTIONS`]. The environment properties an application
/// reads ([`NAMESPACE_APPLICATION_ENVIRONMENT`]) are the other such namespace.
pub(super) const NAMESPACE_TEMPLATE_PARAMETER: &str = "aws:cloudformation:template:parameter";

/// The template parameter whose value is the FAMILY of the instance type the
/// configuration runs on — the one parameter below this service puts a value in
/// force for, derived from the type itself (see [`supplied_option_values`]).
pub(super) const OPTION_INSTANCE_TYPE_FAMILY: &str = "InstanceTypeFamily";

/// The options EVERY platform version SUPPLIES to the environment it provisions
/// rather than publishes for configuration: the parameters of the
/// CloudFormation template Elastic Beanstalk builds for that environment.
///
/// RECORDED BEHAVIOUR, not modelled and not in the published option reference:
/// the awsQuery `DescribeConfigurationSettings` answers in this project's
/// recorded corpus carry these
/// [`NAMESPACE_TEMPLATE_PARAMETER`] entries in the resolved set of a
/// configuration nobody wrote them into, so they belong to the set the platform
/// version puts in force rather than to what a caller configured. They are
/// listed as (namespace, option name) pairs and not as
/// [`ConfigurationOption`]s because a platform publishes no
/// `ConfigurationOptionDescription` for them: there is no default, no change
/// severity and no value set to publish, which is exactly why
/// `DescribeConfigurationOptions` describes none of them.
///
/// They are the parameters of the environment stack every platform version
/// builds: the instance type's family, the proxy's static-file mappings, the
/// environment properties passed to the application, the source of the
/// application version deployed in it and the package of platform hooks its
/// instances run.
pub const PLATFORM_SUPPLIED_OPTIONS: &[(&str, &str)] = &[
    (NAMESPACE_TEMPLATE_PARAMETER, OPTION_INSTANCE_TYPE_FAMILY),
    (NAMESPACE_TEMPLATE_PARAMETER, "StaticFiles"),
    (NAMESPACE_TEMPLATE_PARAMETER, "EnvironmentVariables"),
    (
        NAMESPACE_TEMPLATE_PARAMETER,
        crate::ops::types::RUNTIME_SOURCE_PARAMETER_APP_SOURCE,
    ),
    (NAMESPACE_TEMPLATE_PARAMETER, "HooksPkgUrl"),
];

/// The environment properties a PYTHON platform version supplies on top of
/// those: the module search path its application server reads. Same source as
/// [`PLATFORM_SUPPLIED_OPTIONS`] — a recorded resolved set of a Python
/// configuration carries it.
const PYTHON_SUPPLIED_OPTIONS: &[(&str, &str)] =
    &[(NAMESPACE_APPLICATION_ENVIRONMENT, PYTHON_MODULE_SEARCH_PATH)];

/// The environment property a Python platform version points at the deployed
/// application's directory. See [`SUPPLIED_OPTION_VALUES`].
pub const PYTHON_MODULE_SEARCH_PATH: &str = "PYTHONPATH";

/// The properties a platform version supplies because of the RUNTIME it runs,
/// by the [`SolutionStack::platform_category`] the catalog publishes it under —
/// the same keying [`PLATFORM_CATEGORY_CONFIGURATION_OPTIONS`] uses. A category
/// not listed here supplies none: this emulator has no recorded resolved set
/// for it, and a property guessed from nothing would be reported as the
/// platform's.
const PLATFORM_CATEGORY_SUPPLIED_OPTIONS: &[(&str, &[(&str, &str)])] =
    &[("Python", PYTHON_SUPPLIED_OPTIONS)];

/// The options the platform version `stack` SUPPLIES to a configuration set for
/// it: the template parameters every version supplies, then the properties its
/// runtime adds. See [`PLATFORM_SUPPLIED_OPTIONS`].
pub fn platform_supplied_options(
    stack: Option<&SolutionStack>,
) -> Vec<(&'static str, &'static str)> {
    let runtime = stack
        .and_then(|stack| {
            PLATFORM_CATEGORY_SUPPLIED_OPTIONS
                .iter()
                .find(|(category, _)| *category == stack.platform_category)
                .map(|(_, supplied)| *supplied)
        })
        .unwrap_or_default();
    PLATFORM_SUPPLIED_OPTIONS
        .iter()
        .chain(runtime)
        .copied()
        .collect()
}

/// The configuration options the platform version `stack` defines, in a stable
/// order (by namespace, then by option name) so two reads of one platform
/// report the same catalog in the same sequence.
///
/// The three sources are the general catalog every platform defines, the
/// options the platform's RUNTIME adds, and the worker daemon's options, which
/// a platform defines only where it supports that tier (`supports_worker_tier`,
/// read from the branch catalog). A platform version this service cannot place
/// in its catalog — a CUSTOM one, whose runtime is inside a platform definition
/// archive this emulator does not read — defines the general catalog alone,
/// rather than a runtime's options guessed from nothing.
pub fn published_configuration_options(
    stack: Option<&SolutionStack>,
    supports_worker_tier: bool,
) -> Vec<&'static ConfigurationOption> {
    let runtime = stack
        .map(|stack| {
            PLATFORM_CATEGORY_CONFIGURATION_OPTIONS
                .iter()
                .find(|(category, _)| *category == stack.platform_category)
                .map_or(LINUX_CONFIGURATION_OPTIONS, |(_, options)| *options)
        })
        .unwrap_or_default();
    let worker = if supports_worker_tier {
        WORKER_TIER_CONFIGURATION_OPTIONS
    } else {
        &[]
    };
    let mut options: Vec<&'static ConfigurationOption> = GENERAL_CONFIGURATION_OPTIONS
        .iter()
        .chain(runtime)
        .chain(worker)
        .collect();
    options.sort_by_key(|option| option.key());
    options
}

/// The platform version a configuration set is FOR, from the two forms it is
/// named by: a platform version is named BOTH by its solution stack name and by
/// its ARN, and this service's catalog publishes both, so either form resolves
/// the one published version. A version the catalog does not publish — a CUSTOM
/// one — resolves to none, which is what leaves its catalog the general one.
///
/// The ONE place that resolution is written: every operation that has to know
/// which platform version a stored configuration set belongs to (the option
/// catalog it publishes, the defaults its settings resolve against) reads the
/// same answer.
pub fn configuration_platform(
    region: &str,
    solution_stack_name: Option<&str>,
    platform_arn: Option<&str>,
) -> Option<&'static SolutionStack> {
    // The option catalog is the BRANCH's: every version of a branch publishes
    // the same configuration options, so the branch's catalog row answers for
    // whichever of its versions the request named.
    named_platform_version(region, solution_stack_name, platform_arn).map(|version| version.stack)
}

/// The managed platform version a request names, by either of the two names a
/// platform version has: its `SolutionStackName` or its `PlatformArn`.
///
/// The ONE place that resolution is written, so every operation that reports
/// BOTH names for the version an environment or a configuration runs reports two
/// names of the SAME version — a request naming the release a branch's current
/// version superseded gets that release's solution stack name back, not the
/// newer one's. A `SolutionStackName` names the version
/// `ListAvailableSolutionStacks` offers under it; an ARN names whichever of the
/// branch's published versions it belongs to.
pub fn named_platform_version(
    region: &str,
    solution_stack_name: Option<&str>,
    platform_arn: Option<&str>,
) -> Option<ManagedPlatformVersion> {
    solution_stack_name
        .and_then(solution_stack_named)
        .map(|stack| ManagedPlatformVersion {
            stack,
            platform_version: stack.platform_version.to_string(),
        })
        .or_else(|| platform_arn.and_then(|arn| managed_platform_version(region, arn)))
}

/// The option settings a configuration SET holds, RESOLVED: the options
/// `supplied` by the platform version, then every option the option catalog
/// `catalog` publishes, with the settings `written` on the configuration itself
/// applied over them.
///
/// Elastic Beanstalk resolves a configuration through a layered chain — what the
/// platform version supplies and defaults, then the saved configuration, then
/// the settings set on the environment — so the configuration set a read reports
/// is the EFFECTIVE set of that platform version: it carries every option that
/// version defines or supplies, whoever wrote it, each at the value in force —
/// the setting the configuration itself holds, else the `DefaultValue`
/// `DescribeConfigurationOptions` publishes for the same (`Namespace`,
/// `OptionName`) on that version, else no value at all.
///
/// The `supplied` options are the ones that platform version SUPPLIES to the
/// environment it provisions rather than publishes for configuration (see
/// [`platform_supplied_options`]): they are in the set whether or not a value is
/// in force for them, because the platform puts them there — an option in force
/// at no value is reported with no `Value`, which is also what makes it
/// attributable to no layer (see [`resolved_option_setting_details`]).
///
/// An option the catalog defines no default for is in the set the same way, in
/// force at no value until the configuration sets one: what puts an option in
/// the set is the platform version DEFINING it, not a value standing behind it.
/// An option the catalog does not define at all — an application environment
/// variable, which the caller names — is reported because the configuration sets
/// it.
///
/// This is why the WRITTEN settings are what the store keeps: the override an
/// `OptionsToRemove` takes out stops replacing the default, so that option
/// resolves to the inherited default again rather than disappearing from the
/// set.
pub fn resolved_option_settings(
    catalog: &[&'static ConfigurationOption],
    supplied: &[(&'static str, &'static str)],
    written: &[ConfigurationOptionSetting],
) -> Vec<ConfigurationOptionSetting> {
    // The options the platform SUPPLIES come first, which is where the recorded
    // resolved sets carry them: they are the parameters of the environment the
    // service provisions, not values a caller chose.
    let supplied: Vec<ConfigurationOptionSetting> = supplied
        .iter()
        .map(|(namespace, option_name)| ConfigurationOptionSetting {
            resource_name: None,
            namespace: Some((*namespace).to_string()),
            option_name: Some((*option_name).to_string()),
            // In force at no value until the service puts one in force
            // ([`supplied_option_values`]) or the configuration sets it.
            value: None,
        })
        .collect();
    let defaults: Vec<ConfigurationOptionSetting> = catalog
        .iter()
        .map(|option| ConfigurationOptionSetting {
            // "A unique resource name for the configuration option setting" —
            // only a time-based scaling option is set on a resource of its own,
            // and a catalog default is set on none.
            resource_name: None,
            namespace: Some(option.namespace.to_string()),
            option_name: Some(option.name.to_string()),
            // "DefaultValue: the default value for this configuration option" —
            // the value in force for an option nobody set. An option the
            // version publishes NO default for has none in force, so it is
            // reported with no `Value` rather than with an empty one.
            value: stated(option.default_value),
        })
        .collect();
    let mut resolved = merge_option_settings(&merge_option_settings(&supplied, &defaults), written);
    supplied_option_values(&mut resolved);
    resolved
}

/// Put the values the SERVICE itself derives for the options it supplies in
/// force on the resolved set `resolved`.
///
/// The template parameter [`OPTION_INSTANCE_TYPE_FAMILY`] is the family of the
/// instance type the configuration runs on ([`environment_instance_type`], the
/// same derivation the environment's resources are reported from), so it tracks
/// the type the configuration puts in force rather than being written out. The
/// environment property a runtime supplies is in force at the FIXED value that
/// platform puts there ([`SUPPLIED_OPTION_VALUES`]). A parameter the
/// configuration ITSELF sets keeps the value it was set to: the derivation fills
/// only a parameter in force at no value.
fn supplied_option_values(resolved: &mut [ConfigurationOptionSetting]) {
    let instance_type = environment_instance_type(resolved);
    let family = instance_type_family(&instance_type).to_string();
    for setting in resolved.iter_mut() {
        if setting.value.is_some() {
            continue;
        }
        let named = (
            setting.namespace.as_deref().unwrap_or_default(),
            setting.option_name.as_deref().unwrap_or_default(),
        );
        if named == (NAMESPACE_TEMPLATE_PARAMETER, OPTION_INSTANCE_TYPE_FAMILY) {
            setting.value = Some(family.clone());
            continue;
        }
        if let Some((_, _, value)) = SUPPLIED_OPTION_VALUES
            .iter()
            .find(|(namespace, option_name, _)| (*namespace, *option_name) == named)
        {
            setting.value = Some((*value).to_string());
        }
    }
}

/// The deployed application's own directory on an environment's instances: the
/// directory the platform runs the application version from — the same one this
/// emulator's own application runtime runs it from (see
/// [`super::application_runtime`]) — which is what a runtime that supplies a
/// module search path points it at.
pub const DEPLOYED_APPLICATION_DIRECTORY: &str = "/var/app/current";

/// The values the SERVICE puts in force for the options a platform version
/// SUPPLIES, beside the derived instance-type family: `(Namespace, OptionName,
/// Value)`.
///
/// That a value IS in force for the property a Python runtime supplies is
/// RECORDED BEHAVIOUR (same source as [`PYTHON_SUPPLIED_OPTIONS`]): the recorded
/// resolved set reports the module search path with `OptionSource` `Service`,
/// which is the service having put a value there rather than the property
/// sitting unset. WHICH value is the platform's documented one for that property
/// — the directory it runs the deployed application from
/// ([`DEPLOYED_APPLICATION_DIRECTORY`]), not a value read off the recording.
const SUPPLIED_OPTION_VALUES: &[(&str, &str, &str)] = &[(
    NAMESPACE_APPLICATION_ENVIRONMENT,
    PYTHON_MODULE_SEARCH_PATH,
    DEPLOYED_APPLICATION_DIRECTORY,
)];

/// The FAMILY of the Amazon EC2 instance type `instance_type`: the part before
/// the size, which is how an instance type is named (`<family>.<size>`). A name
/// carrying no size is its own family.
fn instance_type_family(instance_type: &str) -> &str {
    instance_type
        .split_once('.')
        .map_or(instance_type, |(family, _)| family)
}

/// `OptionSource$Origin` (and `$Identifier`) for a value in force because the
/// PLATFORM VERSION defines that default — nobody set it.
///
/// RECORDED BEHAVIOUR, not modelled: the awsQuery
/// `ConfigurationSettingsDescription` carries one `OptionSettingDetails` entry
/// per described option, naming where its effective value came from, and the
/// recorded entries carry `Service` for a platform default and `API` for a
/// value a request set.
pub const OPTION_SOURCE_SERVICE: &str = "Service";
/// `OptionSource$Origin` (and `$Identifier`) for a value the configuration set
/// itself holds — one a request wrote through the API. See
/// [`OPTION_SOURCE_SERVICE`].
pub const OPTION_SOURCE_API: &str = "API";

/// Where each option of a resolved configuration set got its effective value:
/// one entry per setting [`resolved_option_settings`] reports, in the same
/// order, so the two lists line up position for position.
///
/// The source is the layer the value in force came from: an option the
/// configuration set WROTE is in force because a request set it
/// ([`OPTION_SOURCE_API`]), and one in force because the platform version
/// defines that default or supplies that value is the SERVICE's
/// ([`OPTION_SOURCE_SERVICE`]). This is the only thing that tells a value a
/// caller set apart from an inherited default, which is what a client reads to
/// confirm a setting it applied took effect.
///
/// An option the set reports at NO value came from NO layer, so its
/// `OptionSource` NAMES none: the member is still there, as the empty structure,
/// because every entry of the recorded `OptionSettingDetails` list carries one —
/// which is how the recorded answers report the options a platform version
/// supplies without a value (see [`PLATFORM_SUPPLIED_OPTIONS`]).
pub fn resolved_option_setting_details(
    catalog: &[&'static ConfigurationOption],
    supplied: &[(&'static str, &'static str)],
    written: &[ConfigurationOptionSetting],
) -> Vec<OptionSettingDetail> {
    let set_through_the_api: BTreeSet<(String, String, String)> =
        written.iter().map(option_setting_key).collect();
    resolved_option_settings(catalog, supplied, written)
        .iter()
        .map(|setting| {
            let origin = if set_through_the_api.contains(&option_setting_key(setting)) {
                OPTION_SOURCE_API
            } else {
                OPTION_SOURCE_SERVICE
            };
            // Both members name the layer the value in force came from, and
            // both are left unstated where no value is in force.
            let source = setting.value.is_some().then(|| origin.to_string());
            OptionSettingDetail {
                // The three members that NAME the option, the triple an
                // `OptionSpecification` names one by, so a detail about an
                // option set on a resource of its own says which resource.
                resource_name: setting.resource_name.clone(),
                namespace: setting.namespace.clone(),
                option_name: setting.option_name.clone(),
                option_source: Some(OptionSource {
                    identifier: source.clone(),
                    origin: source,
                }),
            }
        })
        .collect()
}
