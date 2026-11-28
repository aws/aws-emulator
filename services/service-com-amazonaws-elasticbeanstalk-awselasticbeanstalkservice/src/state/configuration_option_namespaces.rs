// SPDX-License-Identifier: CC0-1.0

//! Which configuration NAMESPACE an option specification can name — the
//! question a submitted `(Namespace, OptionName)` pair is resolved against
//! before any value in it is evaluated.
//!
//! A `ConfigurationOptionSetting` is "a specification identifying an individual
//! configuration option along with its current value", named by its
//! `Namespace` — "a unique namespace identifying the option's associated Amazon
//! Web Services resource" — and the option's name inside it. A namespace no
//! platform version defines identifies no resource and so no configuration
//! option at all: a specification naming one cannot be resolved, which is why
//! it is reported as an unknown configuration setting rather than evaluated,
//! and why a write carrying it is refused (see
//! [`super::configuration_setting_validation`]).
//!
//! The resolution is made at NAMESPACE granularity, against every namespace any
//! platform version this service publishes can define
//! ([`every_publishable_option`]) plus the ones named below. The option tables
//! of [`super::configuration_option_catalog`] carry a DESCRIPTION for the
//! options this emulator serves behaviour for, which is a subset of what a
//! platform version's namespaces hold: the namespaces whose option names the
//! CALLER chooses carry no description at all, and the ones belonging to a
//! resource this emulator does not provision carry none here either. Whether an
//! individual option NAME is one the namespace holds is therefore a different
//! question, and one this resolution does not answer — it is asked of the
//! catalog a platform version PUBLISHES, by the evaluation in
//! [`super::configuration_setting_validation`], and only of a namespace that
//! catalog states the names of: a namespace it describes no option of states
//! none, so asking it there would refuse settings the real service accepts.
//!
//! Split out of [`super`] so the one walk over every option table any platform
//! version can publish lives beside the namespaces those tables do not carry.

use super::*;

/// The configuration namespaces an Elastic Beanstalk platform version defines
/// that this service's option catalog publishes no description of, as the
/// service's own published configuration-option reference names them (its
/// "general options for all environments" namespace list, which
/// [`super::configuration_option_catalog`] draws the described options from).
///
/// Each entry is a namespace whose options are the caller's OWN names (the
/// environment properties and the template parameters passed to the
/// application), are scoped to a resource of their own (a scheduled action), or
/// belong to a resource this emulator provisions no behaviour for (the database
/// instance, the proxy's static files, the traffic split of an immutable
/// deployment, and the load balancers' listeners and policies). The platform
/// resolves a specification naming one of them, so this service does too.
pub const UNDESCRIBED_CONFIGURATION_NAMESPACES: &[&str] = &[
    "aws:autoscaling:scheduledaction",
    "aws:cloudformation:template:parameter",
    "aws:elasticbeanstalk:application:environment",
    "aws:elasticbeanstalk:environment:proxy:staticfiles",
    "aws:elasticbeanstalk:trafficsplitting",
    "aws:elb:listener",
    "aws:elb:policies",
    "aws:elbv2:loadbalancer",
    "aws:rds:dbinstance",
];

/// The namespaces whose name ENDS in a part the caller chooses: the environment
/// process a request is routed to, the load-balancer port a listener serves,
/// and the name of a listener rule or of a legacy load-balancer policy. A
/// specification names one of them by writing that part after the prefix, so
/// what the platform defines is the prefix followed by a name — the prefix with
/// nothing after it names no resource.
///
/// Same source as [`UNDESCRIBED_CONFIGURATION_NAMESPACES`].
pub const CONFIGURATION_NAMESPACE_PREFIXES: &[&str] = &[
    "aws:elasticbeanstalk:environment:process:",
    "aws:elb:listener:",
    "aws:elb:policies:",
    "aws:elbv2:listener:",
    "aws:elbv2:listenerrule:",
];

/// The namespaces whose option NAME is a value the CALLER chooses rather than one
/// the platform version defines a fixed set of.
///
/// A static-file mapping is named by the virtual path it serves — "the option
/// name is the virtual path, and the value is the directory of the source
/// bundle it is served from" — so one such namespace holds one option per path a
/// configuration maps, and the option reference's own entry for it (`/static`)
/// is the mapping a platform version ships rather than the set of names the
/// namespace admits. Asking the catalog whether it holds a NAME of one of these
/// would therefore refuse every mapping but that one, which the real service
/// accepts.
///
/// A namespace listed in [`UNDESCRIBED_CONFIGURATION_NAMESPACES`] is already
/// outside that question (the catalog describes no option of it), so the entry
/// here for the proxy's own static files states the rule for the class rather
/// than changing an answer; the runtime's namespace is described, and is the one
/// this rule decides.
pub const CALLER_NAMED_OPTION_NAMESPACES: &[&str] = &[
    "aws:elasticbeanstalk:container:python:staticfiles",
    "aws:elasticbeanstalk:environment:proxy:staticfiles",
];

/// Whether the option NAMES of `namespace` are the caller's own
/// ([`CALLER_NAMED_OPTION_NAMESPACES`]), so no set of names can be stated for it.
#[must_use]
pub fn namespace_names_its_own_options(namespace: &str) -> bool {
    CALLER_NAMED_OPTION_NAMESPACES.contains(&namespace)
}

/// Whether an Elastic Beanstalk platform version defines the configuration
/// namespace `namespace` — the one place that question is answered, so every
/// operation that takes option settings resolves a specification the same way.
///
/// A namespace is one a platform version defines when any option table this
/// service can publish carries an option in it, when it is one of the
/// namespaces those tables describe no option of
/// ([`UNDESCRIBED_CONFIGURATION_NAMESPACES`]), or when it is one of the
/// prefixes above followed by the name of the resource it is scoped to. A
/// setting carrying no namespace names none of them.
pub fn platform_defines_namespace(namespace: &str) -> bool {
    if namespace.is_empty() {
        return false;
    }
    every_publishable_option().any(|option| option.namespace == namespace)
        || UNDESCRIBED_CONFIGURATION_NAMESPACES.contains(&namespace)
        || CONFIGURATION_NAMESPACE_PREFIXES.iter().any(|prefix| {
            namespace
                .strip_prefix(prefix)
                .is_some_and(|scoped| !scoped.is_empty())
        })
}

/// Every option any platform version this service publishes can define: the
/// general catalog, the Amazon SQS daemon's, each platform category's runtime
/// options and the proxy option of a platform whose category names none. The one
/// walk over all of the catalog's tables, so a question asked of "what a
/// platform version can define" is asked of the whole published set rather than
/// of a sample of it.
pub fn every_publishable_option() -> impl Iterator<Item = &'static ConfigurationOption> {
    use super::configuration_option_catalog::{
        LINUX_CONFIGURATION_OPTIONS, PLATFORM_CATEGORY_CONFIGURATION_OPTIONS,
    };
    GENERAL_CONFIGURATION_OPTIONS
        .iter()
        .chain(WORKER_TIER_CONFIGURATION_OPTIONS)
        .chain(LINUX_CONFIGURATION_OPTIONS)
        .chain(
            PLATFORM_CATEGORY_CONFIGURATION_OPTIONS
                .iter()
                .flat_map(|(_, options)| options.iter()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a namespace a platform version defines is resolved; one no platform
    /// version defines is not.
    ///
    /// Rows differ along WHY the namespace is defined — described by an option
    /// table, described by none, scoped to a resource named after a prefix — and
    /// along how a namespace that is not defined misses: a service prefix with a
    /// name no platform holds, a near miss of a described namespace, a scoped
    /// prefix with nothing after it, and a name of no such shape at all.
    #[test]
    fn a_namespace_is_resolved_only_where_a_platform_version_defines_it() {
        let cases = [
            ("an option table describes it", "aws:autoscaling:asg", true),
            (
                "a runtime's own table describes it",
                "aws:elasticbeanstalk:container:tomcat:jvmoptions",
                true,
            ),
            (
                "the worker daemon's table describes it",
                "aws:elasticbeanstalk:sqsd",
                true,
            ),
            (
                "its option names are the caller's own",
                "aws:elasticbeanstalk:application:environment",
                true,
            ),
            (
                "its options are scoped to a resource of their own",
                "aws:autoscaling:scheduledaction",
                true,
            ),
            (
                "no option table describes it",
                "aws:rds:dbinstance",
                true,
            ),
            (
                "a prefix followed by the resource it is scoped to",
                "aws:elbv2:listener:443",
                true,
            ),
            (
                "another prefix followed by its resource",
                "aws:elasticbeanstalk:environment:process:api",
                true,
            ),
            (
                "a name no platform version defines",
                "aws:elasticbeanstalk:namespace-no-platform-defines",
                false,
            ),
            (
                "a near miss of a described namespace",
                "aws:autoscaling:asgroup",
                false,
            ),
            ("a scoped prefix with nothing after it", "aws:elbv2:listener:", false),
            ("not a namespace shape at all", "my-own-settings", false),
            ("no namespace at all", "", false),
        ];

        for (case, namespace, defined) in cases {
            assert_eq!(
                platform_defines_namespace(namespace),
                defined,
                "case {case:?}: expected {namespace:?} to be {}, got {}",
                if defined { "defined" } else { "undefined" },
                platform_defines_namespace(namespace)
            );
        }
    }

    /// RULE: the namespaces listed here are the ones no option table describes,
    /// so a namespace that gains a described option is taken off the list rather
    /// than answered from two places.
    #[test]
    fn the_listed_namespaces_are_the_ones_no_option_table_describes() {
        for namespace in UNDESCRIBED_CONFIGURATION_NAMESPACES {
            assert!(
                !every_publishable_option().any(|option| option.namespace == *namespace),
                "case {namespace:?}: expected no published option in a namespace listed as \
                 described by none"
            );
        }
    }
}
