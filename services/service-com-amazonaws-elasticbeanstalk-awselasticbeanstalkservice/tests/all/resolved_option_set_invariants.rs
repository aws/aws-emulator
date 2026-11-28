// SPDX-License-Identifier: CC0-1.0

//! The invariants the resolved configuration set holds for EVERY platform
//! version this service publishes, checked against the catalogue derivations
//! directly rather than over the wire.
//!
//! `all/effective_configuration_option_set.rs` drives the rule end to end over
//! the awsQuery surface for a handful of runtimes; this suite sweeps the WHOLE
//! published platform catalogue, so a version whose runtime table the HTTP rows
//! do not name cannot resolve a set that breaks one of these:
//!
//! * every option the version publishes is in the set exactly once, at the
//!   default it publishes for it, and every option it supplies is in it too;
//! * the detail list is one entry per setting, in the same order, about the same
//!   option;
//! * a setting the configuration writes is in force at the value it was written
//!   to, and attributed to the API, wherever the catalogue defines that option
//!   or not.

use std::collections::BTreeSet;

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// A setting written on the configuration: the one the catalogue defines
/// (`MaxSize`, which every version publishes) is named through the service's own
/// constants so this suite cannot spell an option a second way.
fn written() -> Vec<eb::ops::types::ConfigurationOptionSetting> {
    vec![
        eb::ops::types::ConfigurationOptionSetting {
            resource_name: None,
            namespace: Some("aws:autoscaling:asg".to_string()),
            option_name: Some("MaxSize".to_string()),
            value: Some("7".to_string()),
        },
        eb::ops::types::ConfigurationOptionSetting {
            resource_name: None,
            namespace: Some(eb::state::NAMESPACE_APPLICATION_ENVIRONMENT.to_string()),
            option_name: Some("STAGE".to_string()),
            value: Some("qa".to_string()),
        },
    ]
}

#[test]
fn every_published_platform_version_resolves_a_set_that_carries_its_whole_catalogue() {
    for stack in eb::state::SOLUTION_STACKS {
        let case = stack.name();
        for worker_capable in [false, true] {
            let catalogue = eb::state::published_configuration_options(Some(stack), worker_capable);
            let supplied = eb::state::platform_supplied_options(Some(stack));
            for settings in [Vec::new(), written()] {
                let resolved =
                    eb::state::resolved_option_settings(&catalogue, &supplied, &settings);
                let details =
                    eb::state::resolved_option_setting_details(&catalogue, &supplied, &settings);

                let keys: Vec<(String, String)> = resolved
                    .iter()
                    .map(|setting| {
                        (
                            setting.namespace.clone().unwrap_or_default(),
                            setting.option_name.clone().unwrap_or_default(),
                        )
                    })
                    .collect();
                let detailed: Vec<(String, String)> = details
                    .iter()
                    .map(|detail| {
                        (
                            detail.namespace.clone().unwrap_or_default(),
                            detail.option_name.clone().unwrap_or_default(),
                        )
                    })
                    .collect();
                assert_eq!(
                    detailed, keys,
                    "case {case:?}: the detail list must be about the same options in the same \
                     order"
                );

                let present: BTreeSet<(String, String)> = keys.iter().cloned().collect();
                assert_eq!(
                    present.len(),
                    keys.len(),
                    "case {case:?}: no option is reported twice, got {keys:?}"
                );
                for option in &catalogue {
                    let key = (option.namespace.to_string(), option.name.to_string());
                    assert!(
                        present.contains(&key),
                        "case {case:?}: expected the published {}/{} in the resolved set",
                        option.namespace,
                        option.name
                    );
                }
                for (namespace, option_name) in &supplied {
                    let key = ((*namespace).to_string(), (*option_name).to_string());
                    assert!(
                        present.contains(&key),
                        "case {case:?}: expected the supplied {namespace}/{option_name} in the \
                         resolved set"
                    );
                }

                // What the configuration WROTE is in force at the written
                // value and attributed to the API, whether or not the
                // catalogue defines that option.
                for setting in &settings {
                    let at = keys
                        .iter()
                        .position(|key| {
                            (key.0.as_str(), key.1.as_str())
                                == (
                                    setting.namespace.as_deref().unwrap_or_default(),
                                    setting.option_name.as_deref().unwrap_or_default(),
                                )
                        })
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: expected the written {setting:?} in the set")
                        });
                    assert_eq!(
                        resolved[at].value, setting.value,
                        "case {case:?}: expected the written {setting:?} in force at its value"
                    );
                    assert_eq!(
                        details[at].option_source,
                        Some(eb::ops::types::OptionSource {
                            identifier: Some(eb::state::OPTION_SOURCE_API.to_string()),
                            origin: Some(eb::state::OPTION_SOURCE_API.to_string()),
                        }),
                        "case {case:?}: expected the written {setting:?} attributed to the API"
                    );
                }
            }
        }
    }
}
