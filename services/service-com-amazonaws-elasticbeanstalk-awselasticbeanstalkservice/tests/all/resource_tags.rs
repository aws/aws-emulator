// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` over this service's awsQuery surface: the tags a resource
//! was created with, read back by that resource's ARN, and the refusals an ARN that
//! names no taggable resource earns.
//!
//! The rule under test: the operation reports the tags the store holds for the
//! Elastic Beanstalk resource the ARN's resource segment names — a value that is not
//! an ARN of this service is refused with `InvalidParameterValue`, an Elastic
//! Beanstalk ARN of a type the service does not tag with
//! `ResourceTypeNotSupportedException`, and one of a taggable type naming no stored
//! resource with `ResourceNotFoundException`. Each table row is a different point in
//! that class (resource type, tag count, key/value shape and length, ARN form), not
//! a renamed copy of the one before it.
//!
//! The HTTP harness (`Emulator`, `Answer`) is the one `lifecycle.rs` already owns:
//! nothing here reaches into the store directly, so a tag that is not persisted, or
//! a list served from anything other than the store, fails these tests. One test
//! asserts the awsQuery envelope ESCAPES a tag or an ARN carrying XML
//! metacharacters, over the raw document rather than through the harness's reader,
//! so a value reflected unescaped fails it.

use crate::lifecycle::{
    account, wait_for_status, Answer, Emulator, SOLUTION_STACK, TEST_REGION,
};

/// The key/value pairs `ListTagsForResource` reported, sorted so the assertion is
/// about the SET of tags on the resource and not about a list order the model does
/// not promise. Each element's text is read with the XML entities resolved, so a tag
/// carrying `&`, `<`, `>`, `"` or `'` is compared against what was sent and an
/// envelope that failed to escape one is caught rather than silently reparsed.
pub(crate) fn listed_tags(answer: &Answer) -> Vec<(String, String)> {
    let keys = answer.values_at("ResourceTags/member/Key");
    let values = answer.values_at("ResourceTags/member/Value");
    assert_eq!(
        keys.len(),
        values.len(),
        "every reported tag carries a Key and a Value, got body {}",
        answer.body
    );
    let mut pairs: Vec<(String, String)> = keys
        .into_iter()
        .zip(values)
        .map(|(key, value)| (xml_unescaped(key), xml_unescaped(value)))
        .collect();
    pairs.sort();
    pairs
}

/// `text` with the five predefined XML entities resolved. The harness reads the raw
/// slice between two tags, so this is what turns an escaped element back into the
/// value the caller supplied.
pub(crate) fn xml_unescaped(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        // Last, so an escaped entity in the ORIGINAL value is not resolved twice.
        .replace("&amp;", "&")
}

/// `tags` sorted the same way, for comparison with [`listed_tags`].
pub(crate) fn expected_tags(tags: &[(String, String)]) -> Vec<(String, String)> {
    let mut pairs = tags.to_vec();
    pairs.sort();
    pairs
}

/// The awsQuery form parameters that apply `tags` at creation
/// (`Tags.member.<n>.Key` / `Tags.member.<n>.Value`: the `Tags` list's member is
/// unnamed in the model, so it serializes as `member`).
pub(crate) fn tag_params(tags: &[(String, String)]) -> Vec<(String, String)> {
    let mut params = Vec::with_capacity(tags.len() * 2);
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push((format!("Tags.member.{position}.Key"), key.clone()));
        params.push((format!("Tags.member.{position}.Value"), value.clone()));
    }
    params
}

/// Borrow an owned parameter list for [`Emulator::call`].
pub(crate) fn as_params(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// A tag pair, as the table rows write them.
pub(crate) fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One row of a tag table: a resource to create with `tags`, under a case name.
struct Row {
    case: &'static str,
    /// The resource's own name — an application name, a version label or an
    /// environment name, depending on which table the row is in.
    name: &'static str,
    /// The region the creating call is signed for, which the resource's ARN then
    /// carries.
    region: &'static str,
    tags: Vec<(String, String)>,
}

/// A row created in [`TEST_REGION`].
fn row(case: &'static str, name: &'static str, tags: Vec<(String, String)>) -> Row {
    Row {
        case,
        name,
        region: TEST_REGION,
        tags,
    }
}

/// A row created in `region`.
fn row_in(
    case: &'static str,
    name: &'static str,
    region: &'static str,
    tags: Vec<(String, String)>,
) -> Row {
    Row {
        case,
        name,
        region,
        tags,
    }
}

/// Ask for `arn`'s tags and assert they are exactly `tags`, that the answer echoes
/// the ARN asked about, and that asking twice answers the same — a read must not
/// change what the next caller sees.
pub(crate) async fn assert_tags(
    emulator: &Emulator,
    case: &str,
    arn: &str,
    tags: &[(String, String)],
) {
    let listed = emulator
        .call("ListTagsForResource", &[("ResourceArn", arn)])
        .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource for {arn:?}, got {} and body {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.value("ResourceArn"),
        Some(arn),
        "case {case:?}: the answer must name the ARN asked about, got body {}",
        listed.body
    );
    assert_eq!(
        listed_tags(&listed),
        expected_tags(tags),
        "case {case:?}: unexpected tags for {arn:?} in body {}",
        listed.body
    );

    let again = emulator
        .call("ListTagsForResource", &[("ResourceArn", arn)])
        .await;
    assert_eq!(
        listed_tags(&again),
        expected_tags(tags),
        "case {case:?}: a second read must answer the same, got body {}",
        again.body
    );
}

/// Assert `arn` is refused with HTTP 400 and error code `code`.
async fn assert_refused(emulator: &Emulator, case: &str, arn: &str, code: &str) {
    let refused = emulator
        .call("ListTagsForResource", &[("ResourceArn", arn)])
        .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400 for {arn:?}, got {} and body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        code,
        "case {case:?}: expected error code {code} for {arn:?}, got body {}",
        refused.body
    );
    assert!(
        refused.value("Error/Message").is_some(),
        "case {case:?}: a refusal must say what is wrong, got body {}",
        refused.body
    );
}

#[tokio::test]
async fn application_tags_are_reported_for_the_application_they_were_set_on() {
    let emulator = Emulator::new();

    // Rows differ along tag count and along key/value shape and length. The
    // length bounds are the model's own: `TagKey` is length 1..128 and `TagValue`
    // 1..256 (smithy_query shape="Tag").
    let rows = vec![
        row("no tags", "untagged-app", Vec::new()),
        row("one tag", "one-tag-app", vec![tag("Name", "frontend")]),
        row(
            "three tags of mixed shapes",
            "many-tags-app",
            vec![
                tag("env", "prod"),
                tag("cost-center", "1234"),
                tag("owner:team", "plateforme-données"),
            ],
        ),
        row(
            "keys and values at both modelled length bounds",
            "bounded-tags-app",
            vec![tag("k", "v"), tag(&"K".repeat(128), &"V".repeat(256))],
        ),
        // The model constrains a tag key and value by LENGTH only (smithy_query
        // shape="Tag"), so a value carrying XML metacharacters is a value the
        // operation must report back intact through the awsQuery envelope.
        row(
            "a key and a value carrying XML metacharacters",
            "escaped-tags-app",
            vec![tag("owner&team", "a<b> & \"c\" 'd'")],
        ),
    ];

    for Row {
        case, name, tags, ..
    } in &rows
    {
        let mut params = vec![("ApplicationName".to_string(), (*name).to_string())];
        params.extend(tag_params(tags));
        let created = emulator
            .call("CreateApplication", &as_params(&params))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateApplication, got {} and body {}",
            created.status, created.body
        );
        let arn = created
            .expect("Application/ApplicationArn", case)
            .to_string();
        assert_tags(&emulator, case, &arn, tags).await;
    }

    // The resource segment is what identifies the resource: the emulator serves one
    // account, so an ARN that differs only in partition, region and account names
    // the same application.
    let one_tag = &rows[1];
    let elsewhere =
        "arn:aws-us-gov:elasticbeanstalk:eu-west-1:999999999999:application/one-tag-app";
    assert_tags(&emulator, one_tag.case, elsewhere, &one_tag.tags).await;
}

#[tokio::test]
async fn a_tag_carrying_xml_metacharacters_is_escaped_in_the_envelope() {
    let emulator = Emulator::new();
    // Rows that would BREAK the awsQuery envelope if a value were reflected into
    // it unescaped: the five predefined entities' characters, and a payload that
    // closes and reopens the element it is written into.
    let tags = vec![
        tag("owner&team", "a<b> & \"c\" 'd'"),
        tag("injection", "</Value><Value>second"),
    ];
    let mut params = vec![(
        "ApplicationName".to_string(),
        "escaped-envelope-app".to_string(),
    )];
    params.extend(tag_params(&tags));
    let created = emulator
        .call("CreateApplication", &as_params(&params))
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );
    let arn = created
        .expect("Application/ApplicationArn", "escaped envelope")
        .to_string();

    let case = "a tag carrying XML metacharacters";
    let listed = emulator
        .call("ListTagsForResource", &[("ResourceArn", arn.as_str())])
        .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {} and body {}",
        listed.status, listed.body
    );
    // The raw document carries the ESCAPED forms, and none of the raw payloads:
    // this is what a serializer that reflected the value unescaped would fail.
    for escaped in ["&amp;", "&lt;", "&gt;"] {
        assert!(
            listed.body.contains(escaped),
            "case {case:?}: the envelope must escape metacharacters as {escaped}, got body {}",
            listed.body
        );
    }
    for raw in ["a<b>", "</Value><Value>second", "owner&team"] {
        assert!(
            !listed.body.contains(raw),
            "case {case:?}: the envelope must not carry {raw:?} unescaped, got body {}",
            listed.body
        );
    }
    // And the document still holds exactly the two tags, each reported with the
    // value it was created with: an unescaped injection payload would have split
    // one element into more.
    assert_eq!(
        listed.values_at("ResourceTags/member/Value").len(),
        tags.len(),
        "case {case:?}: expected {} reported values, got body {}",
        tags.len(),
        listed.body
    );
    assert_eq!(
        listed_tags(&listed),
        expected_tags(&tags),
        "case {case:?}: unexpected tags in body {}",
        listed.body
    );
}

#[tokio::test]
async fn an_application_version_reports_its_own_tags_not_the_applications() {
    let emulator = Emulator::new();
    let application_tags = vec![tag("scope", "application")];
    let mut params = vec![("ApplicationName".to_string(), "versioned".to_string())];
    params.extend(tag_params(&application_tags));
    let created = emulator
        .call("CreateApplication", &as_params(&params))
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );
    let application_arn = created
        .expect("Application/ApplicationArn", "versioned application")
        .to_string();

    // Rows differ along tag count and label shape.
    let rows = vec![
        row("a version with no tags", "v1", Vec::new()),
        row(
            "a version with two tags",
            "release-2.0.1",
            vec![tag("scope", "version"), tag("build", "42")],
        ),
    ];

    for Row {
        case,
        name: label,
        tags,
        ..
    } in &rows
    {
        let mut params = vec![
            ("ApplicationName".to_string(), "versioned".to_string()),
            ("VersionLabel".to_string(), (*label).to_string()),
            ("SourceBundle.S3Bucket".to_string(), "bundles".to_string()),
            ("SourceBundle.S3Key".to_string(), format!("{label}.zip")),
        ];
        params.extend(tag_params(tags));
        let version = emulator
            .call("CreateApplicationVersion", &as_params(&params))
            .await;
        assert_eq!(
            version.status, 200,
            "case {case:?}: expected 200 from CreateApplicationVersion, got {} and body {}",
            version.status, version.body
        );
        let arn = version
            .expect("ApplicationVersion/ApplicationVersionArn", case)
            .to_string();
        assert_tags(&emulator, case, &arn, tags).await;
    }

    // "Elastic Beanstalk applies these tags only to the application version" — the
    // application still reports its own.
    assert_tags(
        &emulator,
        "the application beside its versions",
        &application_arn,
        &application_tags,
    )
    .await;
}

#[tokio::test]
async fn an_environment_reports_the_tags_it_was_created_with() {
    let emulator = Emulator::new();
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", "tagged-envs")])
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );

    // Rows differ along tag count and along the region the environment was created
    // in, which its ARN carries.
    let rows = vec![
        row("an environment with no tags", "plain-env", Vec::new()),
        row(
            "an environment with two tags",
            "tagged-env",
            vec![tag("tier", "web"), tag("env", "staging")],
        ),
        row_in(
            "an environment created in another region",
            "other-region-env",
            "ap-southeast-2",
            vec![tag("tier", "worker")],
        ),
    ];

    for Row {
        case,
        name: environment,
        region,
        tags,
    } in &rows
    {
        let mut params = vec![
            ("ApplicationName".to_string(), "tagged-envs".to_string()),
            ("EnvironmentName".to_string(), (*environment).to_string()),
            (
                "SolutionStackName".to_string(),
                SOLUTION_STACK.to_string(),
            ),
        ];
        params.extend(tag_params(tags));
        let launched = emulator
            .call_in_region(region, "CreateEnvironment", &as_params(&params))
            .await;
        assert_eq!(
            launched.status, 200,
            "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
            launched.status, launched.body
        );
        let arn = launched.expect("EnvironmentArn", case).to_string();
        assert_eq!(
            arn,
            format!(
                "arn:aws:elasticbeanstalk:{region}:{}:environment/tagged-envs/{environment}",
                account()
            ),
            "case {case:?}: unexpected EnvironmentArn in {}",
            launched.body
        );
        assert_tags(&emulator, case, &arn, tags).await;
    }
}

#[tokio::test]
async fn an_arn_that_names_no_taggable_resource_is_refused() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator
            .call(
                "CreateApplication",
                &[
                    ("ApplicationName", "exists"),
                    ("Tags.member.1.Key", "owner"),
                    ("Tags.member.1.Value", "platform"),
                ],
            )
            .await
            .status,
        200,
        "expected 200 from CreateApplication"
    );
    assert_eq!(
        emulator
            .call(
                "CreateApplicationVersion",
                &[
                    ("ApplicationName", "exists"),
                    ("VersionLabel", "v1"),
                    ("SourceBundle.S3Bucket", "bundles"),
                    ("SourceBundle.S3Key", "v1.zip"),
                ],
            )
            .await
            .status,
        200,
        "expected 200 from CreateApplicationVersion"
    );
    assert_eq!(
        emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", "exists"),
                    ("EnvironmentName", "live-env"),
                    (
                        "SolutionStackName",
                        SOLUTION_STACK.as_str(),
                    ),
                ],
            )
            .await
            .status,
        200,
        "expected 200 from CreateEnvironment"
    );

    let account = account();
    let eb = format!("arn:aws:elasticbeanstalk:{TEST_REGION}:{account}");
    // The resource types Elastic Beanstalk tags — application, application
    // version, configuration template, environment and custom platform version
    // (`crate::state::TAGGABLE_RESOURCE_TYPES`, from "Tagging Application
    // Resources", the doc `ListTagsForResource` points at). `solutionstack` is an
    // Elastic Beanstalk ARN type that is NOT one of them; a value that is not an
    // ARN of this service at all is a bad parameter rather than an unsupported
    // type.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "another namespace's ARN",
            "arn:aws:s3:::some-bucket".to_string(),
            "InvalidParameterValue",
        ),
        (
            "an Elastic Beanstalk type that is not tagged",
            format!("{eb}:solutionstack/64bit Amazon Linux 2 v5.8.0 running Node.js 18"),
            "ResourceTypeNotSupportedException",
        ),
        (
            "an unknown Elastic Beanstalk resource type",
            format!("{eb}:widget/exists"),
            "ResourceTypeNotSupportedException",
        ),
        (
            "a string that is not an ARN",
            "exists".to_string(),
            "InvalidParameterValue",
        ),
        (
            "an ARN with an empty resource segment",
            format!("{eb}:"),
            "InvalidParameterValue",
        ),
        (
            "an application that was never created",
            format!("{eb}:application/never-created"),
            "ResourceNotFoundException",
        ),
        (
            "an application ARN with an empty name",
            format!("{eb}:application/"),
            "ResourceNotFoundException",
        ),
        (
            "an application ARN with an extra name segment",
            format!("{eb}:application/exists/extra"),
            "ResourceNotFoundException",
        ),
        (
            "a version label the application does not have",
            format!("{eb}:applicationversion/exists/v9"),
            "ResourceNotFoundException",
        ),
        (
            "a version under an application that does not exist",
            format!("{eb}:applicationversion/never-created/v1"),
            "ResourceNotFoundException",
        ),
        (
            "an environment name that was never used",
            format!("{eb}:environment/exists/never-launched"),
            "ResourceNotFoundException",
        ),
        (
            "an environment under a different application",
            format!("{eb}:environment/never-created/live-env"),
            "ResourceNotFoundException",
        ),
        (
            "a configuration template this application does not hold",
            format!("{eb}:configurationtemplate/exists/base-template"),
            "ResourceNotFoundException",
        ),
        (
            "a custom platform version this account never created",
            format!("{eb}:platform/custom-platform/1.0.3"),
            "ResourceNotFoundException",
        ),
        (
            "an application name carrying XML metacharacters",
            format!("{eb}:application/a&b<c>\"d\""),
            "ResourceNotFoundException",
        ),
    ];

    for (case, arn, code) in &rows {
        assert_refused(&emulator, case, arn, code).await;
    }

    // The refusal envelope is built from the value the caller sent, so one
    // carrying XML metacharacters must come back as a parseable envelope that
    // still names the ARN to fix.
    let case = "an application name carrying XML metacharacters";
    let hostile = format!("{eb}:application/a&b<c>\"d\"");
    let refused = emulator
        .call("ListTagsForResource", &[("ResourceArn", hostile.as_str())])
        .await;
    let message = xml_unescaped(refused.expect("Error/Message", case));
    assert!(
        message.contains(&hostile),
        "case {case:?}: the refusal must name the ARN asked about, got message {message:?} in body {}",
        refused.body
    );
    // The error envelope escapes it too, so the document stays well formed: the raw
    // payload must not appear, and its escaped form must.
    assert!(
        refused.body.contains("&lt;") && refused.body.contains("&amp;"),
        "case {case:?}: the error envelope must escape the ARN's metacharacters, got body {}",
        refused.body
    );
    assert!(
        !refused.body.contains("a&b<c>"),
        "case {case:?}: the error envelope must not carry the ARN unescaped, got body {}",
        refused.body
    );

    // The positive control beside them: the resources that DO exist answer 200,
    // each with the tags IT carries — the application's tag is not on the version
    // or the environment created under it.
    for (case, arn, tags) in [
        (
            "the application",
            format!("{eb}:application/exists"),
            vec![tag("owner", "platform")],
        ),
        (
            "the application version",
            format!("{eb}:applicationversion/exists/v1"),
            Vec::new(),
        ),
        (
            "the environment",
            format!("{eb}:environment/exists/live-env"),
            Vec::new(),
        ),
    ] {
        assert_tags(&emulator, case, &arn, &tags).await;
    }
}

#[tokio::test]
async fn a_terminated_environment_is_no_longer_a_taggable_resource() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator
            .call("CreateApplication", &[("ApplicationName", "short-lived")])
            .await
            .status,
        200,
        "expected 200 from CreateApplication"
    );
    let launched = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "short-lived"),
                ("EnvironmentName", "doomed-env"),
                (
                    "SolutionStackName",
                    SOLUTION_STACK.as_str(),
                ),
                ("Tags.member.1.Key", "tier"),
                ("Tags.member.1.Value", "web"),
            ],
        )
        .await;
    assert_eq!(
        launched.status, 200,
        "expected 200 from CreateEnvironment, got {} and body {}",
        launched.status, launched.body
    );
    let arn = launched.expect("EnvironmentArn", "doomed-env").to_string();
    let tags = vec![tag("tier", "web")];
    assert_tags(&emulator, "while launching", &arn, &tags).await;

    // A termination can only take down an environment whose launch has got past
    // its initial phase, which is what the clock owes it: waited for as a
    // CONDITION, never as a fixed time.
    wait_for_status(&emulator, "doomed-env", "doomed-env", "Ready").await;
    let terminated = emulator
        .call("TerminateEnvironment", &[("EnvironmentName", "doomed-env")])
        .await;
    assert_eq!(
        terminated.status, 200,
        "expected 200 from TerminateEnvironment, got {} and body {}",
        terminated.status, terminated.body
    );
    // Still terminating, so the environment is still there to be read.
    assert_tags(&emulator, "while terminating", &arn, &tags).await;

    // Once the clock has made the move to `Terminated` due, the environment is
    // gone and its ARN names no resource — waited for as the CONDITION the
    // environment reaches, never as a fixed time.
    wait_for_status(&emulator, "doomed-env", "doomed-env", "Terminated").await;
    assert_refused(
        &emulator,
        "once terminated",
        &arn,
        "ResourceNotFoundException",
    )
    .await;
}
