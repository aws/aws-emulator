// SPDX-License-Identifier: CC0-1.0

//! The shapes reached by more than one operation whose WIRE carries members the
//! on-disk model does not declare.
//!
//! WHY THEY ARE NOT IN `shared_types`. That module is rendered from the model by
//! the scaffold and is rewritten whole by the next model refresh, so a member the
//! model does not declare cannot survive there. These declarations are
//! hand-written — the module carries no `@generated` marker — so a refresh leaves
//! them alone, and `shared_types` keeps the shapes whose member list IS the
//! model's. `types/mod.rs` re-exports this module like every other, so
//! `use super::types::*` resolves these shapes wherever they are reached and the
//! generated shapes that merely NAME them (`EnvironmentDescriptionsMessage`'s
//! list, `ConfigurationSettingsDescriptions`' list) keep resolving unchanged.
//!
//! See `types::unmodelled_member_wire` for why a structure carrying such a member
//! cannot be derived with `#[smithy_macros::smithy(shape = ...)]` at all, and what
//! the macro this module builds on emits in the derive's place.
//!
//! What each shape carries beyond the model:
//!
//! * `CreateEnvironmentOutput` — the `EnvironmentDescription` of `CreateEnvironment`
//!   and the element of `DescribeEnvironments`/`ComposeEnvironments`' list. Its two
//!   unmodelled members and the member list it shares with the per-operation
//!   structures are `types::environment_description_wire`'s.
//! * `CreateConfigurationTemplateOutput` — the `ConfigurationSettingsDescription` of
//!   `CreateConfigurationTemplate` and the element of `DescribeConfigurationSettings`'
//!   list. RECORDED BEHAVIOUR, not modelled: a template WRITE reports `TemplateUrl`,
//!   the stored template document's Amazon S3 location, and a READ of a configuration
//!   set reports `OptionSettingDetails` (where each resolved option's value came
//!   from), `EnvironmentLinkPoints` and the `Tier` the configuration is for. Each
//!   handler fills in the ones its own response carries.
//!   `UpdateConfigurationTemplateOutput`, which reports `TemplateUrl` alone, states
//!   the same list in `types::update_configuration_template_types`.
//! * `OptionSettingDetail` and `OptionSource` — the element of `OptionSettingDetails`
//!   and the source it names. The model declares neither shape, so both are declared
//!   here, beside the only shape that reaches them.
#![allow(unused_imports)]

use super::*;

// The shared `EnvironmentDescription` structure: one member list, a struct per
// operation the model binds the shape to.
eb_environment_description!(CreateEnvironmentOutput, CREATE_ENVIRONMENT_OUTPUT_SCHEMA);

/// Where ONE option of a resolved configuration set got its effective value: the
/// triple that names the option and the source the value came from.
///
/// An option in force at NO value came from no layer, so it carries no
/// `OptionSource` — see
/// [`crate::state::resolved_option_setting_details`], which is where the values
/// come from.
///
/// Comparable by value, so a caller of that derivation can assert the detail it
/// produced for one option against the detail that option must carry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptionSettingDetail {
    pub resource_name: Option<String>,
    pub namespace: Option<String>,
    pub option_name: Option<String>,
    pub option_source: Option<OptionSource>,
}

eb_schema_and_codec!(
    OptionSettingDetail,
    OPTION_SETTING_DETAIL_SCHEMA,
    "com.amazonaws.elasticbeanstalk#OptionSettingDetail",
    "com.amazonaws.elasticbeanstalk",
    "OptionSettingDetail",
    (
        M_RESOURCE_NAME,
        resource_name,
        "ResourceName",
        0,
        opt,
        string
    ),
    (M_NAMESPACE, namespace, "Namespace", 1, opt, string),
    (M_OPTION_NAME, option_name, "OptionName", 2, opt, string),
    (
        M_OPTION_SOURCE,
        option_source,
        "OptionSource",
        3,
        opt,
        struct_of(OptionSource)
    ),
);

/// The layer an option's effective value came from, as one
/// [`OptionSettingDetail`] names it. Comparable by value, for the same reason.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptionSource {
    pub identifier: Option<String>,
    pub origin: Option<String>,
}

eb_schema_and_codec!(
    OptionSource,
    OPTION_SOURCE_SCHEMA,
    "com.amazonaws.elasticbeanstalk#OptionSource",
    "com.amazonaws.elasticbeanstalk",
    "OptionSource",
    (M_IDENTIFIER, identifier, "Identifier", 0, opt, string),
    (M_ORIGIN, origin, "Origin", 1, opt, string),
);

/// What `CreateConfigurationTemplate` reports and what one element of a
/// `DescribeConfigurationSettings` answer carries: the modelled
/// `ConfigurationSettingsDescription`, plus the four members this module's header
/// describes.
///
/// Member set and order from
/// `smithy_query(query="shape", shape="ConfigurationSettingsDescription")`, with the
/// unmodelled four appended last.
#[derive(Debug, Clone, Default)]
pub struct CreateConfigurationTemplateOutput {
    pub solution_stack_name: Option<String>,
    pub platform_arn: Option<String>,
    pub application_name: Option<String>,
    pub template_name: Option<String>,
    pub description: Option<String>,
    pub environment_name: Option<String>,
    pub deployment_status: Option<String>,
    pub date_created: Option<::aws_smithy_types::DateTime>,
    pub date_updated: Option<::aws_smithy_types::DateTime>,
    pub option_settings: Option<Vec<ConfigurationOptionSetting>>,
    pub template_url: Option<String>,
    pub option_setting_details: Option<Vec<OptionSettingDetail>>,
    pub environment_link_points: Option<String>,
    pub tier: Option<EnvironmentTier>,
}

eb_schema_and_codec!(
    CreateConfigurationTemplateOutput,
    CREATE_CONFIGURATION_TEMPLATE_OUTPUT_SCHEMA,
    "com.amazonaws.elasticbeanstalk#ConfigurationSettingsDescription",
    "com.amazonaws.elasticbeanstalk",
    "ConfigurationSettingsDescription",
    (
        M_SOLUTION_STACK_NAME,
        solution_stack_name,
        "SolutionStackName",
        0,
        opt,
        string
    ),
    (M_PLATFORM_ARN, platform_arn, "PlatformArn", 1, opt, string),
    (
        M_APPLICATION_NAME,
        application_name,
        "ApplicationName",
        2,
        opt,
        string
    ),
    (
        M_TEMPLATE_NAME,
        template_name,
        "TemplateName",
        3,
        opt,
        string
    ),
    (M_DESCRIPTION, description, "Description", 4, opt, string),
    (
        M_ENVIRONMENT_NAME,
        environment_name,
        "EnvironmentName",
        5,
        opt,
        string
    ),
    (
        M_DEPLOYMENT_STATUS,
        deployment_status,
        "DeploymentStatus",
        6,
        opt,
        string
    ),
    (
        M_DATE_CREATED,
        date_created,
        "DateCreated",
        7,
        opt,
        date_time
    ),
    (
        M_DATE_UPDATED,
        date_updated,
        "DateUpdated",
        8,
        opt,
        date_time
    ),
    (
        M_OPTION_SETTINGS,
        option_settings,
        "OptionSettings",
        9,
        opt,
        list_of_struct(ConfigurationOptionSetting)
    ),
    (M_TEMPLATE_URL, template_url, "TemplateUrl", 10, opt, string),
    (
        M_OPTION_SETTING_DETAILS,
        option_setting_details,
        "OptionSettingDetails",
        11,
        opt,
        list_of_struct(OptionSettingDetail)
    ),
    (
        M_ENVIRONMENT_LINK_POINTS,
        environment_link_points,
        "EnvironmentLinkPoints",
        12,
        opt,
        string
    ),
    (M_TIER, tier, "Tier", 13, opt, struct_of(EnvironmentTier)),
);
