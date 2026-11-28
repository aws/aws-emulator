// SPDX-License-Identifier: CC0-1.0

//! The description text one published configuration option carries.
//!
//! `ConfigurationOptionDescription` has no `Description` member in the on-disk
//! model, and the awsQuery responses of the real service carry one on every
//! option description: the option reference's own sentence about what that
//! option does. The text is service-owned reference data, exactly as the
//! defaults and the restrictions in [`super::configuration_option_catalog`]
//! are, so it is held here as one table keyed by the pair that identifies an
//! option — `(Namespace, OptionName)` — rather than threaded through that
//! module's builders.
//!
//! SOURCE: Elastic Beanstalk's published configuration-option reference (the
//! "General options for all environments" and per-platform tables), the same
//! reference the namespaces, the defaults and the value sets beside it come
//! from. The two Auto Scaling group sizes' sentences are the ones the recorded
//! wire responses carry verbatim.
//!
//! The unit test at the bottom pins the invariant that makes the member
//! reportable at all: every option any platform version this service publishes
//! defines has a sentence here, so no published option description goes out
//! without one.

use super::*;

/// The description of one option: `(Namespace, OptionName, Description)`.
///
/// Keyed by the pair [`ConfigurationOption::key`] is, so one option name
/// appearing in two namespaces (`SecurityGroups`, `RetentionInDays`,
/// `DeleteOnTerminate`, `Timeout`) carries the sentence of the resource THAT
/// namespace configures.
type OptionDescription = (&'static str, &'static str, &'static str);

/// What each option the catalog publishes does. See the module note for the
/// source.
pub(super) const OPTION_DESCRIPTIONS: &[OptionDescription] = &[
    // aws:autoscaling:asg — the Auto Scaling group the instances run in.
    (
        NAMESPACE_AUTOSCALING_ASG,
        "Availability Zones",
        "Number of Availability Zones you want to run the environment's instances in.",
    ),
    (
        NAMESPACE_AUTOSCALING_ASG,
        "Cooldown",
        "Number of seconds to wait after one scaling activity completes before any further \
         scaling activity can start.",
    ),
    (
        NAMESPACE_AUTOSCALING_ASG,
        OPTION_MAX_SIZE,
        "Maximum number of instances you want running. Must be a positive integer that is less \
         than 10000.",
    ),
    (
        NAMESPACE_AUTOSCALING_ASG,
        OPTION_MIN_SIZE,
        "Minimum number of instances you want running. Must be a positive integer no greater \
         than MaxSize.",
    ),
    // aws:autoscaling:trigger — the CloudWatch alarm the group scales on.
    (
        NAMESPACE_AUTOSCALING_TRIGGER,
        "BreachDuration",
        "Amount of time, in minutes, a metric can be beyond its defined limit (as specified in \
         the UpperThreshold and LowerThreshold) before the trigger fires.",
    ),
    (
        NAMESPACE_AUTOSCALING_TRIGGER,
        "EvaluationPeriods",
        "Number of consecutive evaluation periods used to determine if a breach is occurring.",
    ),
    (
        NAMESPACE_AUTOSCALING_TRIGGER,
        "LowerBreachScaleIncrement",
        "How many Amazon EC2 instances to remove when performing a scaling activity.",
    ),
    (
        NAMESPACE_AUTOSCALING_TRIGGER,
        "LowerThreshold",
        "If the measurement falls below this number for the breach duration, a trigger is fired.",
    ),
    (
        NAMESPACE_AUTOSCALING_TRIGGER,
        "MeasureName",
        "Metric used for your Auto Scaling trigger.",
    ),
    (
        NAMESPACE_AUTOSCALING_TRIGGER,
        "Period",
        "Specifies how frequently Amazon CloudWatch measures the metrics for your trigger.",
    ),
    (
        NAMESPACE_AUTOSCALING_TRIGGER,
        "Statistic",
        "Statistic the trigger should use, such as Average.",
    ),
    (
        NAMESPACE_AUTOSCALING_TRIGGER,
        "Unit",
        "Unit for the trigger measurement, such as Bytes.",
    ),
    (
        NAMESPACE_AUTOSCALING_TRIGGER,
        "UpperBreachScaleIncrement",
        "How many Amazon EC2 instances to add when performing a scaling activity.",
    ),
    (
        NAMESPACE_AUTOSCALING_TRIGGER,
        "UpperThreshold",
        "If the measurement is higher than this number for the breach duration, a trigger is \
         fired.",
    ),
    // aws:autoscaling:launchconfiguration — the instances themselves.
    (
        NAMESPACE_LAUNCH_CONFIGURATION,
        "DisableIMDSv1",
        "Whether to disable version 1 of the Amazon EC2 Instance Metadata Service on the \
         environment's instances.",
    ),
    (
        NAMESPACE_LAUNCH_CONFIGURATION,
        "EC2KeyName",
        "Name of the Amazon EC2 key pair to log in to the environment's instances with.",
    ),
    (
        NAMESPACE_LAUNCH_CONFIGURATION,
        "IamInstanceProfile",
        "Instance profile that gives the environment's instances the permissions they need.",
    ),
    (
        NAMESPACE_LAUNCH_CONFIGURATION,
        OPTION_INSTANCE_TYPE,
        "Amazon EC2 instance type the environment's instances run on.",
    ),
    (
        NAMESPACE_LAUNCH_CONFIGURATION,
        "MonitoringInterval",
        "Interval at which Amazon CloudWatch metrics are collected from the environment's \
         instances.",
    ),
    (
        NAMESPACE_LAUNCH_CONFIGURATION,
        "RootVolumeSize",
        "Size, in GB, of the root volume attached to each of the environment's instances.",
    ),
    (
        NAMESPACE_LAUNCH_CONFIGURATION,
        "RootVolumeType",
        "Type of the root volume attached to each of the environment's instances.",
    ),
    (
        NAMESPACE_LAUNCH_CONFIGURATION,
        "SecurityGroups",
        "Amazon EC2 security groups to assign to the environment's instances.",
    ),
    // aws:autoscaling:updatepolicy:rollingupdate — how a configuration change
    // is rolled out across the group.
    (
        "aws:autoscaling:updatepolicy:rollingupdate",
        "RollingUpdateEnabled",
        "Whether a configuration change is applied to the environment's instances in batches \
         rather than to all of them at once.",
    ),
    (
        "aws:autoscaling:updatepolicy:rollingupdate",
        "RollingUpdateType",
        "Kind of rolling update to apply when the environment's configuration changes: by time, \
         by health, or immutable.",
    ),
    // aws:ec2:instances — the instance types the group may launch.
    (
        NAMESPACE_EC2_INSTANCES,
        "EnableSpot",
        "Whether the environment's Auto Scaling group may launch Spot Instances.",
    ),
    (
        NAMESPACE_EC2_INSTANCES,
        OPTION_INSTANCE_TYPES,
        "Amazon EC2 instance types the environment's Auto Scaling group may launch.",
    ),
    (
        NAMESPACE_EC2_INSTANCES,
        "SupportedArchitectures",
        "Architectures of the Amazon EC2 instance types the environment may launch.",
    ),
    // aws:ec2:vpc — where the environment runs.
    (
        "aws:ec2:vpc",
        "AssociatePublicIpAddress",
        "Whether to launch the environment's instances with public IP addresses.",
    ),
    (
        "aws:ec2:vpc",
        "ELBScheme",
        "Whether the environment's load balancer is internet-facing or internal to the VPC.",
    ),
    (
        "aws:ec2:vpc",
        "ELBSubnets",
        "IDs of the subnets the environment's load balancer is placed in.",
    ),
    (
        "aws:ec2:vpc",
        "Subnets",
        "IDs of the subnets the environment's instances are launched in.",
    ),
    (
        "aws:ec2:vpc",
        "VPCId",
        "ID of the Amazon VPC the environment runs in.",
    ),
    // aws:elasticbeanstalk:application — the application's health check path.
    (
        "aws:elasticbeanstalk:application",
        "Application Healthcheck URL",
        "Path the load balancer requests to check the health of the application.",
    ),
    // aws:elasticbeanstalk:cloudwatch:logs — instance log streaming.
    (
        "aws:elasticbeanstalk:cloudwatch:logs",
        "DeleteOnTerminate",
        "Whether to delete the instance log groups when the environment is terminated.",
    ),
    (
        "aws:elasticbeanstalk:cloudwatch:logs",
        "RetentionInDays",
        "Number of days to keep the environment's instance logs in CloudWatch Logs.",
    ),
    (
        "aws:elasticbeanstalk:cloudwatch:logs",
        "StreamLogs",
        "Whether to stream the environment's instance logs to CloudWatch Logs.",
    ),
    // aws:elasticbeanstalk:cloudwatch:logs:health — environment health
    // streaming, a second log group with its own retention.
    (
        "aws:elasticbeanstalk:cloudwatch:logs:health",
        "DeleteOnTerminate",
        "Whether to delete the environment health log group when the environment is terminated.",
    ),
    (
        "aws:elasticbeanstalk:cloudwatch:logs:health",
        "HealthStreamingEnabled",
        "Whether to stream the environment's health information to CloudWatch Logs.",
    ),
    (
        "aws:elasticbeanstalk:cloudwatch:logs:health",
        "RetentionInDays",
        "Number of days to keep the environment's health logs in CloudWatch Logs.",
    ),
    // aws:elasticbeanstalk:command — how an application version is deployed.
    (
        "aws:elasticbeanstalk:command",
        "BatchSize",
        "Number or percentage of the environment's instances to deploy to in one batch.",
    ),
    (
        "aws:elasticbeanstalk:command",
        "BatchSizeType",
        "Whether BatchSize is a fixed number of instances or a percentage of them.",
    ),
    (
        "aws:elasticbeanstalk:command",
        "DeploymentPolicy",
        "How a new application version is deployed to the environment's instances.",
    ),
    (
        "aws:elasticbeanstalk:command",
        "IgnoreHealthCheck",
        "Whether to continue a deployment even when the instances fail their health check.",
    ),
    (
        "aws:elasticbeanstalk:command",
        "Timeout",
        "Number of seconds to wait for an instance to finish running the deployment commands.",
    ),
    // aws:elasticbeanstalk:environment — the environment itself.
    (
        NAMESPACE_ENVIRONMENT,
        OPTION_ENVIRONMENT_TYPE,
        "Whether the environment runs behind a load balancer or on a single instance.",
    ),
    (
        NAMESPACE_ENVIRONMENT,
        "LoadBalancerType",
        "Type of load balancer the environment runs behind.",
    ),
    (
        NAMESPACE_ENVIRONMENT,
        "ServiceRole",
        "IAM role Elastic Beanstalk assumes to manage the environment's resources and monitor \
         its health.",
    ),
    // aws:elasticbeanstalk:healthreporting:system — basic or enhanced health.
    (
        "aws:elasticbeanstalk:healthreporting:system",
        "HealthCheckSuccessThreshold",
        "Health level an instance must reach before enhanced health reporting considers it \
         healthy.",
    ),
    (
        "aws:elasticbeanstalk:healthreporting:system",
        "SystemType",
        "Whether the environment reports basic or enhanced health.",
    ),
    // aws:elasticbeanstalk:hostmanager — instance log publication to Amazon S3.
    (
        "aws:elasticbeanstalk:hostmanager",
        "LogPublicationControl",
        "Whether to copy the environment's instance logs to its Amazon S3 bucket.",
    ),
    // aws:elasticbeanstalk:managedactions — managed platform updates.
    (
        NAMESPACE_MANAGED_ACTIONS,
        OPTION_MANAGED_ACTIONS_ENABLED,
        "Whether managed platform updates are enabled for the environment.",
    ),
    (
        NAMESPACE_MANAGED_ACTIONS,
        OPTION_PREFERRED_START_TIME,
        "Maintenance window, in UTC, in which managed actions run, as day:hour:minute.",
    ),
    (
        NAMESPACE_MANAGED_ACTIONS,
        "ServiceRoleForManagedUpdates",
        "IAM role Elastic Beanstalk assumes to perform managed platform updates.",
    ),
    (
        NAMESPACE_MANAGED_ACTIONS_PLATFORM_UPDATE,
        OPTION_INSTANCE_REFRESH_ENABLED,
        "Whether to replace the environment's instances weekly even when no platform update is \
         due.",
    ),
    (
        NAMESPACE_MANAGED_ACTIONS_PLATFORM_UPDATE,
        OPTION_UPDATE_LEVEL,
        "Highest level of platform update a managed update applies.",
    ),
    // aws:elasticbeanstalk:monitoring — what the service does with an instance
    // enhanced health reports as failing.
    (
        "aws:elasticbeanstalk:monitoring",
        "Automatically Terminate Unhealthy Instances",
        "Whether to terminate an instance enhanced health reporting finds unhealthy.",
    ),
    // aws:elasticbeanstalk:sns:topics — environment event notifications.
    (
        NAMESPACE_SNS_TOPICS,
        OPTION_NOTIFICATION_ENDPOINT,
        "Address the environment's event notifications are sent to.",
    ),
    (
        NAMESPACE_SNS_TOPICS,
        OPTION_NOTIFICATION_PROTOCOL,
        "Protocol the environment's event notifications are sent with.",
    ),
    (
        NAMESPACE_SNS_TOPICS,
        OPTION_NOTIFICATION_TOPIC_ARN,
        "ARN of the Amazon SNS topic the environment's event notifications are published to.",
    ),
    (
        NAMESPACE_SNS_TOPICS,
        OPTION_NOTIFICATION_TOPIC_NAME,
        "Name of the Amazon SNS topic to create for the environment's event notifications.",
    ),
    // aws:elasticbeanstalk:xray — the X-Ray daemon.
    (
        "aws:elasticbeanstalk:xray",
        "XRayEnabled",
        "Whether to run the AWS X-Ray daemon on the environment's instances.",
    ),
    // aws:elb:healthcheck / aws:elb:loadbalancer — the classic load balancer.
    (
        "aws:elb:healthcheck",
        "HealthyThreshold",
        "Number of consecutive successful health checks before the load balancer considers an \
         instance healthy.",
    ),
    (
        "aws:elb:healthcheck",
        "Interval",
        "Number of seconds between health checks of the environment's instances.",
    ),
    (
        "aws:elb:healthcheck",
        "Timeout",
        "Number of seconds to wait for a health check response from an instance.",
    ),
    (
        "aws:elb:healthcheck",
        "UnhealthyThreshold",
        "Number of consecutive failed health checks before the load balancer considers an \
         instance unhealthy.",
    ),
    (
        "aws:elb:loadbalancer",
        "CrossZone",
        "Whether the load balancer routes traffic evenly across the environment's Availability \
         Zones.",
    ),
    (
        "aws:elb:loadbalancer",
        "SecurityGroups",
        "Amazon EC2 security groups to assign to the environment's load balancer.",
    ),
    // aws:elasticbeanstalk:sqsd — the worker tier's Amazon SQS daemon.
    (
        "aws:elasticbeanstalk:sqsd",
        "ErrorVisibilityTimeout",
        "Number of seconds to wait before Amazon SQS makes a message whose processing failed \
         visible again.",
    ),
    (
        "aws:elasticbeanstalk:sqsd",
        "HttpConnections",
        "Maximum number of concurrent connections the daemon makes to the application.",
    ),
    (
        "aws:elasticbeanstalk:sqsd",
        "HttpPath",
        "Path the daemon posts each message it takes from the queue to.",
    ),
    (
        "aws:elasticbeanstalk:sqsd",
        "InactivityTimeout",
        "Number of seconds to wait for a response to a message the daemon posted.",
    ),
    (
        "aws:elasticbeanstalk:sqsd",
        "MaxRetries",
        "Number of times the daemon retries a message before moving it to the dead-letter \
         queue.",
    ),
    (
        "aws:elasticbeanstalk:sqsd",
        "MimeType",
        "MIME type of the message the daemon posts to the application.",
    ),
    (
        "aws:elasticbeanstalk:sqsd",
        "RetentionPeriod",
        "Number of seconds a message is kept in the environment's queue.",
    ),
    (
        "aws:elasticbeanstalk:sqsd",
        "VisibilityTimeout",
        "Number of seconds a message the daemon took from the queue stays invisible to other \
         readers.",
    ),
    (
        "aws:elasticbeanstalk:sqsd",
        "WorkerQueueURL",
        "URL of the Amazon SQS queue the daemon reads tasks from.",
    ),
    // The proxy every Linux platform's web tier offers.
    (
        NAMESPACE_PROXY,
        OPTION_PROXY_SERVER,
        "Web server that proxies connections to the application.",
    ),
    // aws:elasticbeanstalk:container:python — the WSGI server.
    (
        "aws:elasticbeanstalk:container:python",
        "NumProcesses",
        "Number of daemon processes that run the WSGI application.",
    ),
    (
        "aws:elasticbeanstalk:container:python",
        "NumThreads",
        "Number of threads each daemon process runs the WSGI application in.",
    ),
    (
        "aws:elasticbeanstalk:container:python",
        "WSGIPath",
        "File that contains the WSGI application.",
    ),
    (
        "aws:elasticbeanstalk:container:python:staticfiles",
        "/static",
        "Directory in the source bundle served at the /static path.",
    ),
    // aws:elasticbeanstalk:container:php:phpini — the php.ini directives.
    (
        "aws:elasticbeanstalk:container:php:phpini",
        "allow_url_fopen",
        "Whether the application may treat a URL as a file.",
    ),
    (
        "aws:elasticbeanstalk:container:php:phpini",
        "composer_options",
        "Options passed to composer install when the application is deployed.",
    ),
    (
        "aws:elasticbeanstalk:container:php:phpini",
        "display_errors",
        "Whether PHP errors are written to the response.",
    ),
    (
        "aws:elasticbeanstalk:container:php:phpini",
        "document_root",
        "Directory in the source bundle that holds the site's root document.",
    ),
    (
        "aws:elasticbeanstalk:container:php:phpini",
        "max_execution_time",
        "Number of seconds a script may run before PHP stops it.",
    ),
    (
        "aws:elasticbeanstalk:container:php:phpini",
        "memory_limit",
        "Amount of memory a script may allocate.",
    ),
    (
        "aws:elasticbeanstalk:container:php:phpini",
        "zlib.output_compression",
        "Whether PHP compresses the response with zlib.",
    ),
    // aws:elasticbeanstalk:container:ruby — the Rack and Rails environments.
    (
        "aws:elasticbeanstalk:container:ruby",
        "RackEnv",
        "Rack environment the application runs in.",
    ),
    (
        "aws:elasticbeanstalk:container:ruby",
        "RailsEnv",
        "Rails environment the application runs in.",
    ),
    // aws:elasticbeanstalk:container:nodejs — how the application is started.
    (
        "aws:elasticbeanstalk:container:nodejs",
        "NodeCommand",
        "Command used to start the Node.js application.",
    ),
    // aws:elasticbeanstalk:container:tomcat:jvmoptions — the JVM.
    (
        "aws:elasticbeanstalk:container:tomcat:jvmoptions",
        "JVM Options",
        "Additional command-line options passed to the JVM that runs the application.",
    ),
    (
        "aws:elasticbeanstalk:container:tomcat:jvmoptions",
        "Xms",
        "Initial heap size of the JVM that runs the application.",
    ),
    (
        "aws:elasticbeanstalk:container:tomcat:jvmoptions",
        "Xmx",
        "Maximum heap size of the JVM that runs the application.",
    ),
    // aws:elasticbeanstalk:container:dotnet:apppool — the IIS application pool.
    (
        "aws:elasticbeanstalk:container:dotnet:apppool",
        "Enable 32-bit Applications",
        "Whether the IIS application pool runs the application in 32-bit mode.",
    ),
    (
        "aws:elasticbeanstalk:container:dotnet:apppool",
        "Target Runtime",
        "Version of the .NET runtime the IIS application pool loads.",
    ),
    // The rest of what the option reference defines in the namespaces above —
    // the options this service honours no behaviour for but a platform version
    // still defines, so a configuration naming one resolves (see the comment on
    // them in [`super::configuration_option_catalog`]).
    (
        NAMESPACE_AUTOSCALING_ASG,
        "Custom Availability Zones",
        "Availability Zones you want to run the environment's instances in.",
    ),
    (
        NAMESPACE_LAUNCH_CONFIGURATION,
        "BlockDeviceMappings",
        "Additional Amazon EBS volumes and instance store volumes to attach to the \
         environment's instances.",
    ),
    (
        NAMESPACE_LAUNCH_CONFIGURATION,
        "DisableDefaultEC2SecurityGroup",
        "Whether to keep the default Amazon EC2 security group off the environment's \
         instances, leaving them only the security groups the configuration names.",
    ),
    (
        NAMESPACE_LAUNCH_CONFIGURATION,
        "ImageId",
        "Amazon Machine Image the environment's instances are launched from, in place of the \
         platform version's own.",
    ),
    (
        NAMESPACE_LAUNCH_CONFIGURATION,
        "RootVolumeIOPS",
        "Input/output operations per second to provision for the root volume attached to each \
         of the environment's instances.",
    ),
    (
        NAMESPACE_LAUNCH_CONFIGURATION,
        "RootVolumeThroughput",
        "Throughput, in MiB/s, to provision for the root volume attached to each of the \
         environment's instances.",
    ),
    (
        NAMESPACE_LAUNCH_CONFIGURATION,
        "SSHSourceRestriction",
        "Addresses that may reach the environment's instances on the SSH port.",
    ),
    (
        "aws:autoscaling:updatepolicy:rollingupdate",
        "MaxBatchSize",
        "Number of instances in each batch a rolling update is applied to.",
    ),
    (
        "aws:autoscaling:updatepolicy:rollingupdate",
        "MinInstancesInService",
        "Minimum number of instances that must be in service while other instances are taken \
         out of service by a rolling update.",
    ),
    (
        "aws:autoscaling:updatepolicy:rollingupdate",
        "PauseTime",
        "Time to wait after each batch of a rolling update before moving on to the next one.",
    ),
    (
        "aws:autoscaling:updatepolicy:rollingupdate",
        "Timeout",
        "Maximum time to wait for a batch of a rolling update to become healthy before the \
         update fails.",
    ),
    (
        "aws:ec2:instances",
        "SpotFleetOnDemandBase",
        "Minimum number of On-Demand Instances the Auto Scaling group launches before it \
         launches any Spot Instances.",
    ),
    (
        "aws:ec2:instances",
        "SpotFleetOnDemandAboveBasePercentage",
        "Percentage of the instances above the On-Demand base that the Auto Scaling group \
         launches as On-Demand Instances.",
    ),
    (
        "aws:ec2:instances",
        "SpotMaxPrice",
        "Maximum price per unit hour, in US dollars, to pay for a Spot Instance.",
    ),
    (
        "aws:ec2:vpc",
        "DBSubnets",
        "Subnets of the virtual private cloud to place the environment's database instance in.",
    ),
    (
        NAMESPACE_ENVIRONMENT,
        "ExternalExtensionsS3Bucket",
        "Amazon S3 bucket holding the configuration files the environment applies beside the \
         ones in the source bundle.",
    ),
    (
        NAMESPACE_ENVIRONMENT,
        "ExternalExtensionsS3Key",
        "Amazon S3 key of the archive of configuration files the environment applies beside \
         the ones in the source bundle.",
    ),
    (
        "aws:elasticbeanstalk:healthreporting:system",
        "ConfigDocument",
        "Document describing the Amazon CloudWatch metrics enhanced health publishes for the \
         environment and its instances.",
    ),
    (
        "aws:elasticbeanstalk:healthreporting:system",
        "EnhancedHealthAuthEnabled",
        "Whether the environment's instances authenticate the enhanced health reports they \
         send to the service.",
    ),
    (
        "aws:elb:healthcheck",
        "Target",
        "Protocol, port and path the load balancer's health check of a registered instance \
         addresses.",
    ),
    (
        "aws:elb:loadbalancer",
        "LoadBalancerHTTPPort",
        "Port the load balancer listens for HTTP connections on, or OFF to listen for none.",
    ),
    (
        "aws:elb:loadbalancer",
        "LoadBalancerHTTPSPort",
        "Port the load balancer listens for HTTPS connections on, or OFF to listen for none.",
    ),
    (
        "aws:elb:loadbalancer",
        "LoadBalancerPortProtocol",
        "Protocol the load balancer speaks on its HTTP listener's port.",
    ),
    (
        "aws:elb:loadbalancer",
        "LoadBalancerSSLPortProtocol",
        "Protocol the load balancer speaks on its HTTPS listener's port.",
    ),
    (
        "aws:elb:loadbalancer",
        "ManagedSecurityGroup",
        "Security group the service manages the load balancer's own ingress rules in.",
    ),
];

/// The description the option reference states for the option
/// `(namespace, option_name)`, if the table above holds one.
///
/// The ONE place that lookup is made, so the option catalog and the published
/// description of an option cannot disagree about which option a sentence
/// belongs to.
pub fn option_description(namespace: &str, option_name: &str) -> Option<&'static str> {
    OPTION_DESCRIPTIONS
        .iter()
        .find(|(table_namespace, table_name, _)| {
            *table_namespace == namespace && *table_name == option_name
        })
        .map(|(_, _, description)| *description)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every option any platform version this service publishes defines carries
    /// a sentence, so no published option description reaches the wire without
    /// its `Description` member.
    #[test]
    fn every_published_option_has_a_description() {
        for stack in SOLUTION_STACKS {
            for worker in [false, true] {
                for option in published_configuration_options(Some(stack), worker) {
                    assert!(
                        option_description(option.namespace, option.name).is_some(),
                        "case {:?}/{:?} (stack {:?}): every published option carries a \
                         description",
                        option.namespace,
                        option.name,
                        stack.name()
                    );
                }
            }
        }
    }

    /// One option name can appear in two namespaces, and each carries the
    /// sentence of the resource ITS namespace configures — so the table is
    /// keyed by the pair and no row is reachable only by accident.
    #[test]
    fn a_name_in_two_namespaces_carries_the_sentence_of_its_own_namespace() {
        let instances = option_description(NAMESPACE_LAUNCH_CONFIGURATION, "SecurityGroups");
        let load_balancer = option_description("aws:elb:loadbalancer", "SecurityGroups");
        assert!(
            instances.is_some() && load_balancer.is_some() && instances != load_balancer,
            "expected a sentence per namespace for SecurityGroups, got {instances:?} and \
             {load_balancer:?}"
        );
        assert_eq!(
            option_description("aws:elasticbeanstalk:container:python", "NoSuchOption"),
            None,
            "an option the table does not hold has no description"
        );
    }
}
