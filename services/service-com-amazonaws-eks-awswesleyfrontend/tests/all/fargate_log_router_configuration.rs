// SPDX-License-Identifier: CC0-1.0

//! The CONFIGURATION the Fargate log router reads out of the `aws-logging` ConfigMap, and the
//! destination each of its outputs names.
//!
//! The RULE under test: **a cluster's Fargate log router is configured by the `aws-logging`
//! ConfigMap of the `aws-observability` namespace, whose `output.conf`, `filters.conf` and
//! `parsers.conf` keys carry the router's own sections. An `[OUTPUT]` section whose `Name` is
//! the CloudWatch Logs plugin names a destination in that service: the `log_group_name`
//! group, a stream named from `log_stream_prefix` (the record's tag appended to it) or from
//! the fixed `log_stream_name`, in the `region` it names, with `auto_create_group` saying
//! whether the router creates the group — and `Match` saying whose records it takes. A
//! section naming another plugin, no log group, a group CloudWatch Logs could not hold, or
//! neither a stream nor a stream prefix names no destination at all, so nothing is delivered
//! through it.**
//!
//! The rows differ along what the rule generalises over: the plugin named (both spellings the
//! CloudWatch Logs output comes in, and a destination of another kind), which keys the
//! section carries and which it leaves out, the case and the spacing they are written in, the
//! `data` key the section lives under, and the `Match` pattern (the published `kube.*`, a bare
//! wildcard, a wildcard in the middle, and a pattern that takes none of a pod's records).
//!
//! SOURCE for the key names and their meanings: the `cloudwatch_logs` output plugin's own
//! documented options, as the Amazon EKS *Fargate logging* guide uses them — not a value this
//! emulator invented.

use std::collections::BTreeMap;

use service_com_amazonaws_eks_awswesleyfrontend::state::kubernetes_fargate_logging::{
    cloudwatch_outputs, container_id, container_log_tag, CloudWatchOutput,
};

/// The `data` key the published example puts the router's outputs under.
const OUTPUT_KEY: &str = "output.conf";

/// One ConfigMap `data` map holding `text` under `key`.
fn data(key: &str, text: &str) -> BTreeMap<String, String> {
    [(key.to_string(), text.to_string())].into_iter().collect()
}

/// What one `[OUTPUT]` section names, as a row states it and as [`carried`] reads it off the
/// destination: the log group, the Region, the stream prefix, the fixed stream name and
/// whether the router creates the group.
type Destination<'held> = (
    &'held str,
    Option<&'held str>,
    &'held str,
    Option<&'held str>,
    bool,
);

/// One row of the configuration table: the configuration text, and the destination it names
/// — `None` for a section that names none.
struct Row {
    case: &'static str,
    /// The `data` key the text is stored under.
    key: &'static str,
    text: &'static str,
    /// The destination the section names ([`Destination`]).
    expected: Option<Destination<'static>>,
}

/// What `output` carries, in the shape a row states.
fn carried(output: &CloudWatchOutput) -> Destination<'_> {
    (
        output.log_group.as_str(),
        output.region.as_deref(),
        output.log_stream_prefix.as_str(),
        output.log_stream_name.as_deref(),
        output.auto_create_group,
    )
}

#[test]
fn the_aws_logging_configuration_names_the_destinations_its_outputs_name() {
    let rows: &[Row] = &[
        Row {
            case: "the published example: the plugin, a group, a prefix and auto_create_group",
            key: OUTPUT_KEY,
            text: "[OUTPUT]\n    Name cloudwatch_logs\n    Match   kube.*\n    region us-west-2\n\
                   \n    log_group_name /fargate/published\n    log_stream_prefix from-fluent-bit-\n\
                   \n    auto_create_group true\n",
            expected: Some((
                "/fargate/published",
                Some("us-west-2"),
                "from-fluent-bit-",
                None,
                true,
            )),
        },
        Row {
            case: "the older spelling of the same output plugin",
            key: OUTPUT_KEY,
            text: "[OUTPUT]\n    Name cloudwatch\n    Match *\n    log_group_name /fargate/older\n\
                   \n    log_stream_prefix p-\n",
            expected: Some(("/fargate/older", None, "p-", None, false)),
        },
        Row {
            case: "a fixed stream name instead of a prefix",
            key: OUTPUT_KEY,
            text: "[OUTPUT]\n    Name cloudwatch_logs\n    log_group_name /fargate/fixed\n\
                   \n    log_stream_name one-stream\n",
            expected: Some(("/fargate/fixed", None, "", Some("one-stream"), false)),
        },
        Row {
            case: "keys and the plugin name written in another case, with comments around them",
            key: OUTPUT_KEY,
            text: "# the router\n[Output]\n    NAME CloudWatch_Logs\n    MATCH kube.*\n\
                   \n    LOG_GROUP_NAME /fargate/cased\n    LOG_STREAM_PREFIX c-\n\
                   \n    AUTO_CREATE_GROUP True\n; trailing comment\n",
            expected: Some(("/fargate/cased", None, "c-", None, true)),
        },
        Row {
            case: "a section under another configuration key the router loads",
            key: "filters.conf",
            text: "[OUTPUT]\n    Name cloudwatch_logs\n    log_group_name /fargate/filters\n\
                   \n    log_stream_prefix f-\n",
            expected: Some(("/fargate/filters", None, "f-", None, false)),
        },
        Row {
            case: "auto_create_group written false",
            key: OUTPUT_KEY,
            text: "[OUTPUT]\n    Name cloudwatch_logs\n    log_group_name /fargate/held\n\
                   \n    log_stream_prefix h-\n    auto_create_group false\n",
            expected: Some(("/fargate/held", None, "h-", None, false)),
        },
        Row {
            case: "a region the configuration spells as no Region name at all",
            key: OUTPUT_KEY,
            text: "[OUTPUT]\n    Name cloudwatch_logs\n    region Not A Region\n\
                   \n    log_group_name /fargate/regionless\n    log_stream_prefix r-\n",
            expected: Some(("/fargate/regionless", None, "r-", None, false)),
        },
        Row {
            case: "a destination of another kind entirely",
            key: OUTPUT_KEY,
            text: "[OUTPUT]\n    Name kinesis_firehose\n    Match kube.*\n\
                   \n    delivery_stream my-stream\n",
            expected: None,
        },
        Row {
            case: "the plugin with no log group named",
            key: OUTPUT_KEY,
            text: "[OUTPUT]\n    Name cloudwatch_logs\n    Match kube.*\n    log_stream_prefix x-\n",
            expected: None,
        },
        Row {
            case: "a log group name CloudWatch Logs could not hold",
            key: OUTPUT_KEY,
            text: "[OUTPUT]\n    Name cloudwatch_logs\n    log_group_name has a space\n\
                   \n    log_stream_prefix x-\n",
            expected: None,
        },
        Row {
            case: "neither a stream nor a stream prefix, which the plugin requires one of",
            key: OUTPUT_KEY,
            text: "[OUTPUT]\n    Name cloudwatch_logs\n    log_group_name /fargate/streamless\n",
            expected: None,
        },
        Row {
            case: "an INPUT section, which names no destination",
            key: OUTPUT_KEY,
            text: "[INPUT]\n    Name tail\n    Path /var/log/containers/*.log\n",
            expected: None,
        },
        Row {
            case: "a section under a key the router does not load",
            key: "notes.txt",
            text: "[OUTPUT]\n    Name cloudwatch_logs\n    log_group_name /fargate/unloaded\n\
                   \n    log_stream_prefix u-\n",
            expected: None,
        },
    ];

    for row in rows {
        let Row {
            case,
            key,
            text,
            expected,
        } = row;
        let outputs = cloudwatch_outputs(&data(key, text));
        match expected {
            Some(expected) => {
                assert_eq!(
                    outputs.len(),
                    1,
                    "case {case:?}: expected one destination, got {outputs:?}"
                );
                let got = carried(&outputs[0]);
                assert_eq!(
                    got, *expected,
                    "case {case:?}: expected {expected:?}, got {got:?}"
                );
            }
            None => assert!(
                outputs.is_empty(),
                "case {case:?}: expected no destination at all, got {outputs:?}"
            ),
        }
    }
}

#[test]
fn every_output_of_one_configuration_is_read_in_the_order_it_states_them() {
    let text = "[OUTPUT]\n    Name cloudwatch_logs\n    log_group_name /fargate/first\n\
                \n    log_stream_prefix a-\n\
                \n[OUTPUT]\n    Name kinesis\n    stream other\n\
                \n[OUTPUT]\n    Name cloudwatch_logs\n    log_group_name /fargate/second\n\
                \n    log_stream_name fixed\n";
    let outputs = cloudwatch_outputs(&data(OUTPUT_KEY, text));
    let groups: Vec<&str> = outputs
        .iter()
        .map(|output| output.log_group.as_str())
        .collect();
    assert_eq!(
        groups,
        vec!["/fargate/first", "/fargate/second"],
        "every CloudWatch Logs destination of one configuration is read, in its own order, \
         got {outputs:?}"
    );
}

#[test]
fn an_outputs_match_decides_whose_records_it_takes() {
    let tag = container_log_tag(
        "default",
        "probe-pod",
        "app",
        &container_id("b9b1a1f0-0000-4000-8000-000000000001", "app"),
    );
    // The tag names the pod, its namespace and the container, which is what a stream named
    // from a prefix carries after it.
    for part in ["kube.", "probe-pod_default_app-", ".log"] {
        assert!(
            tag.contains(part),
            "the tag a container's records carry names {part:?}, got {tag:?}"
        );
    }
    // Two containers of one pod, and one container of two pods, are tagged apart.
    let other_container = container_log_tag(
        "default",
        "probe-pod",
        "sidecar",
        &container_id("b9b1a1f0-0000-4000-8000-000000000001", "sidecar"),
    );
    let other_pod = container_log_tag(
        "default",
        "probe-pod",
        "app",
        &container_id("b9b1a1f0-0000-4000-8000-000000000002", "app"),
    );
    assert!(
        tag != other_container && tag != other_pod,
        "each container of each pod has a tag of its own, got {tag:?}, {other_container:?} and \
         {other_pod:?}"
    );

    // Whether an output TAKES those records, over the pattern forms a `Match` comes in.
    for (case, pattern, takes) in [
        ("the published pattern for a cluster's containers", "kube.*", true),
        ("every tag", "*", true),
        ("a wildcard in the middle of the pattern", "kube.*.log", true),
        ("the whole tag spelled out", tag.as_str(), true),
        ("a pattern of another router's records", "other.*", false),
        ("a prefix that is not this tag's", "kube.var.log.pods.*", false),
        ("a pattern spelled out that is not this tag", "kube.other", false),
    ] {
        let configuration = format!(
            "[OUTPUT]\n    Name cloudwatch_logs\n    Match {pattern}\n\
             \n    log_group_name /fargate/matching\n    log_stream_prefix from-fluent-bit-\n"
        );
        let outputs = cloudwatch_outputs(&data(OUTPUT_KEY, &configuration));
        assert_eq!(
            outputs.len(),
            1,
            "case {case:?}: the configuration names one destination, got {outputs:?}"
        );
        let got = outputs[0].takes(&tag);
        assert_eq!(
            got, takes,
            "case {case:?}: expected Match {pattern:?} to take={takes} the tag {tag:?}, got \
             {got}"
        );
        if got {
            // The stream is the configured prefix with the record's tag appended, which is how
            // the plugin's `log_stream_prefix` builds a stream name.
            let stream = outputs[0].stream_for(&tag);
            assert_eq!(
                stream,
                format!("from-fluent-bit-{tag}"),
                "case {case:?}: expected the prefix followed by the tag, got {stream:?}"
            );
        }
    }

    // An output naming a fixed stream delivers every record it takes to that one stream,
    // whatever the record's tag is.
    let fixed = cloudwatch_outputs(&data(
        OUTPUT_KEY,
        "[OUTPUT]\n    Name cloudwatch_logs\n    log_group_name /fargate/fixed\n\
         \n    log_stream_name one-stream\n",
    ));
    assert_eq!(
        fixed.len(),
        1,
        "the fixed-stream configuration names one destination, got {fixed:?}"
    );
    assert_eq!(
        fixed[0].stream_for(&tag),
        "one-stream",
        "a fixed stream name is the stream whatever the tag is, got {fixed:?}"
    );
}
