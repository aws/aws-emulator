// SPDX-License-Identifier: CC0-1.0

//! The correspondence between a template's spelling of a CloudFront configuration and the
//! API's, driven by declarations rather than by a round trip.
//!
//! What a stack's deployment really does with these types is held to end to end by
//! `core/router/tests/all/cloudformation_provisions_cloudfront_resources.rs`; what is pinned
//! here is the one mapping this module owns.

use super::*;

/// The resource a declaration belongs to, as the walk hands it over.
fn resource(resource_type: &str, properties: Value) -> StoredResource {
    StoredResource {
        logical_id: "TheResource".to_string(),
        resource_type: resource_type.to_string(),
        physical_resource_id: "E1EXAMPLEPHYS".to_string(),
        attributes: BTreeMap::new(),
        metadata: None,
        properties,
        materialised: false,
        deletion_policy: None,
        update_replace_policy: None,
    }
}

/// RULE: where CloudFront's model wraps a list in a `{Quantity, Items}` container, the
/// template declares the list alone — so each declared list is re-wrapped with the element
/// name that model's own list member carries, and `Quantity` is the length the template
/// wrote.
#[test]
fn a_declared_list_is_wrapped_as_the_model_wraps_it() {
    let rows: &[(&str, Value, &str)] = &[
        (
            "the aliases, whose items are CNAMEs",
            serde_json::json!({"Aliases": ["a.example.com", "b.example.com"]}),
            "<Aliases><Quantity>2</Quantity><Items><CNAME>a.example.com</CNAME>\
             <CNAME>b.example.com</CNAME></Items></Aliases>",
        ),
        (
            "an empty list, which is a container holding nothing",
            serde_json::json!({"Aliases": []}),
            "<Aliases><Quantity>0</Quantity><Items></Items></Aliases>",
        ),
        (
            "the trusted key groups, whose container carries Enabled too",
            serde_json::json!({"TrustedKeyGroups": ["K2EXAMPLE"]}),
            "<TrustedKeyGroups><Enabled>true</Enabled><Quantity>1</Quantity>\
             <Items><KeyGroup>K2EXAMPLE</KeyGroup></Items></TrustedKeyGroups>",
        ),
        (
            "a geo restriction, whose locations ARE the container's own Quantity and Items",
            serde_json::json!({"Restrictions": {
                "GeoRestriction": {"RestrictionType": "whitelist", "Locations": ["DE", "FR"]}
            }}),
            "<Restrictions><GeoRestriction><RestrictionType>whitelist</RestrictionType>\
             <Quantity>2</Quantity><Items><Location>DE</Location>\
             <Location>FR</Location></Items></GeoRestriction></Restrictions>",
        ),
        (
            "an origin's custom headers, which the API spells CustomHeaders",
            serde_json::json!({"OriginCustomHeaders": [
                {"HeaderName": "X-From", "HeaderValue": "cfn"}
            ]}),
            "<CustomHeaders><Quantity>1</Quantity><Items><OriginCustomHeader>\
             <HeaderName>X-From</HeaderName><HeaderValue>cfn</HeaderValue>\
             </OriginCustomHeader></Items></CustomHeaders>",
        ),
        (
            "a container a template already spells with Quantity and Items",
            serde_json::json!({"StatusCodes": {"Quantity": 2, "Items": [500, 502]}}),
            "<StatusCodes><Quantity>2</Quantity><Items><StatusCode>500</StatusCode>\
             <StatusCode>502</StatusCode></Items></StatusCodes>",
        ),
    ];
    for (case, declared, expected) in rows {
        let members = declared.as_object().expect("a declared structure");
        let actual: String = members
            .iter()
            .map(|(member, value)| rendered_member(member, value, members, "DistributionConfig"))
            .collect();
        assert_eq!(
            actual, *expected,
            "case {case:?}: expected {expected}, got {actual}"
        );
    }
}

/// RULE: `CachedMethods` is a sibling of `AllowedMethods` in a template and a MEMBER of it in
/// the API, so it is written where the API takes it and nowhere else.
#[test]
fn the_cached_methods_are_written_inside_the_allowed_methods() {
    let declared = serde_json::json!({
        "AllowedMethods": ["GET", "HEAD", "OPTIONS"],
        "CachedMethods": ["GET", "HEAD"]
    });
    let members = declared.as_object().expect("a declared behaviour");
    let actual = rendered_structure("DefaultCacheBehavior", members);
    let expected = "<AllowedMethods><Quantity>3</Quantity><Items><Method>GET</Method>\
                    <Method>HEAD</Method><Method>OPTIONS</Method></Items><CachedMethods>\
                    <Quantity>2</Quantity><Items><Method>GET</Method><Method>HEAD</Method>\
                    </Items></CachedMethods></AllowedMethods>";
    assert_eq!(
        actual, expected,
        "case \"a behaviour naming both\": expected {expected}, got {actual}"
    );
}

/// RULE: a template's `Logging` carries no `Enabled` member — declaring a bucket to log into
/// IS enabling logging — so the member the API requires is written from the bucket the
/// declaration names.
#[test]
fn logging_is_enabled_by_the_bucket_the_declaration_names() {
    let rows: &[(&str, Value, &str)] = &[
        (
            "a bucket to log into",
            serde_json::json!({"Bucket": "logs.s3.amazonaws.com", "Prefix": "cdn/"}),
            "<Enabled>true</Enabled><Bucket>logs.s3.amazonaws.com</Bucket><Prefix>cdn/</Prefix>",
        ),
        (
            "no bucket at all",
            serde_json::json!({"Prefix": ""}),
            "<Enabled>false</Enabled><Prefix></Prefix>",
        ),
        (
            "a declaration that states Enabled itself",
            serde_json::json!({"Enabled": false, "Bucket": "logs.s3.amazonaws.com"}),
            "<Enabled>false</Enabled><Bucket>logs.s3.amazonaws.com</Bucket>",
        ),
    ];
    for (case, declared, expected) in rows {
        let members = declared.as_object().expect("a declared logging configuration");
        let actual = rendered_structure("Logging", members);
        assert_eq!(
            actual, *expected,
            "case {case:?}: expected {expected}, got {actual}"
        );
    }
}

/// RULE: the configuration reaches the API in the order that shape declares its members, with
/// the `CallerReference` CloudFormation supplies first and the `Comment` the API requires
/// present even where the template states none.
#[test]
fn the_configuration_carries_the_members_the_api_requires() {
    let declared = resource(
        CLOUDFRONT_DISTRIBUTION,
        serde_json::json!({"DistributionConfig": {
            "Enabled": true,
            "Origins": [{"Id": "primary", "DomainName": "origin.example.com"}],
            "DefaultCacheBehavior": {
                "TargetOriginId": "primary",
                "ViewerProtocolPolicy": "allow-all"
            },
            "IPV6Enabled": true
        }}),
    );
    let rendered = distribution_config_members(&declared.properties, &declared, &[])
        .expect("a configuration this module maps");
    for expected in [
        "<Origins><Quantity>1</Quantity><Items><Origin>",
        "<Comment></Comment>",
        "<Enabled>true</Enabled>",
        "<IsIPV6Enabled>true</IsIPV6Enabled>",
    ] {
        assert!(
            rendered.contains(expected),
            "case \"a minimal configuration\": expected it to carry {expected}, got {rendered}"
        );
    }
    assert!(
        rendered.starts_with("<CallerReference>"),
        "case \"a minimal configuration\": expected the caller reference first, got {rendered}"
    );
    // The caller reference is DERIVED from the stack and the logical id, so the same
    // declaration builds the same one twice — which is what makes a retried deployment the
    // same request and an update's reference the one the create used.
    let again = distribution_config_members(&declared.properties, &declared, &[])
        .expect("a configuration this module maps");
    assert_eq!(
        rendered, again,
        "case \"the same declaration twice\": expected the same document, got {again}"
    );
}

/// A declared configuration nested one level past [`MAX_DECLARED_DEPTH`]: the rendering walks
/// a declaration recursively, so the depth a template can drive is bounded before anything is
/// rendered.
fn nested_beyond_the_bound() -> Value {
    let mut nested = Value::String("Enabled".to_string());
    for _ in 0..=MAX_DECLARED_DEPTH {
        let mut level = Map::new();
        level.insert("Origins".to_string(), nested);
        nested = Value::Object(level);
    }
    nested
}

/// RULE: a property no call here applies is REFUSED rather than dropped, naming it — a
/// distribution reported created with less than its declaration asked for is the shallow
/// success this module exists to avoid.
#[test]
fn an_unapplied_property_is_refused() {
    let rows: &[(&str, &str, Value, &str)] = &[
        (
            "a property of the type nothing here applies",
            CLOUDFRONT_DISTRIBUTION,
            serde_json::json!({"DistributionConfig": {"Enabled": true}, "Unheard": 1}),
            "Unheard",
        ),
        (
            "a legacy member of the configuration the API does not take",
            CLOUDFRONT_DISTRIBUTION,
            serde_json::json!({"DistributionConfig": {"Enabled": true, "CNAMEs": ["a"]}}),
            "CNAMEs",
        ),
        (
            "a member name that is no API member name at all",
            CLOUDFRONT_DISTRIBUTION,
            serde_json::json!({"DistributionConfig": {
                "Enabled": true,
                "Origins": [{"Id": "primary", "<Injected/>": "x"}]
            }}),
            "Injected",
        ),
        (
            "a property of the origin access control nothing here applies",
            CLOUDFRONT_ORIGIN_ACCESS_CONTROL,
            serde_json::json!({"OriginAccessControlConfig": {"Name": "oac"}, "Unheard": 1}),
            "Unheard",
        ),
        (
            "a declaration nested deeper than this emulator carries",
            CLOUDFRONT_DISTRIBUTION,
            serde_json::json!({"DistributionConfig": nested_beyond_the_bound()}),
            "nests deeper",
        ),
    ];
    for (case, resource_type, properties, named) in rows {
        let stored = resource(resource_type, properties.clone());
        let published = if *resource_type == CLOUDFRONT_DISTRIBUTION {
            DISTRIBUTION_PROPERTIES
        } else {
            ORIGIN_ACCESS_CONTROL_PROPERTIES
        };
        let refusal = refuse_unapplied_properties(&stored.properties, resource_type, published)
            .err()
            .or_else(|| {
                if *resource_type == CLOUDFRONT_DISTRIBUTION {
                    distribution_config_members(&stored.properties, &stored, &[]).err()
                } else {
                    origin_access_control_document(&stored.properties, &stored).err()
                }
            })
            .unwrap_or_else(|| panic!("case {case:?}: expected a refusal"));
        assert!(
            refusal.message.contains(named),
            "case {case:?}: expected the refusal to name {named:?}, got {:?}",
            refusal.message
        );
    }
}

/// RULE: an origin access control's configuration is its create's own members, and the `Name`
/// the API requires falls back to the identifier CloudFormation computed for the resource.
#[test]
fn the_origin_access_control_configuration_is_the_creates_own_members() {
    let rows: &[(&str, Value, &str)] = &[
        (
            "a full declaration",
            serde_json::json!({"OriginAccessControlConfig": {
                "Name": "tenant-oac",
                "Description": "the control of a tenant",
                "SigningProtocol": "sigv4",
                "SigningBehavior": "always",
                "OriginAccessControlOriginType": "s3"
            }}),
            "<Name>tenant-oac</Name><Description>the control of a tenant</Description>\
             <SigningProtocol>sigv4</SigningProtocol><SigningBehavior>always</SigningBehavior>\
             <OriginAccessControlOriginType>s3</OriginAccessControlOriginType>",
        ),
        (
            "a declaration that names no Name",
            serde_json::json!({"OriginAccessControlConfig": {
                "SigningProtocol": "sigv4",
                "SigningBehavior": "always",
                "OriginAccessControlOriginType": "s3"
            }}),
            "<Name>E1EXAMPLEPHYS</Name><SigningProtocol>sigv4</SigningProtocol>\
             <SigningBehavior>always</SigningBehavior>\
             <OriginAccessControlOriginType>s3</OriginAccessControlOriginType>",
        ),
    ];
    for (case, properties, expected) in rows {
        let stored = resource(CLOUDFRONT_ORIGIN_ACCESS_CONTROL, properties.clone());
        let document = origin_access_control_document(&stored.properties, &stored)
            .unwrap_or_else(|refusal| panic!("case {case:?}: expected a document, got {refusal:?}"));
        assert!(
            document.contains(expected),
            "case {case:?}: expected it to carry {expected}, got {document}"
        );
    }
}

/// RULE: the version a mutation of a CloudFront resource is conditional on travels in the
/// answer's `ETag` header, and the configuration's OWN `Enabled` member is the one that says
/// whether the distribution is enabled — not the `Enabled` of a member that has one of its
/// own.
#[test]
fn the_configurations_own_enabled_is_the_one_that_is_read_and_written() {
    let config = "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/2020-05-31/\">\
                  <CallerReference>r</CallerReference>\
                  <DefaultCacheBehavior><TrustedSigners><Enabled>false</Enabled>\
                  <Quantity>0</Quantity></TrustedSigners></DefaultCacheBehavior>\
                  <Logging><Enabled>true</Enabled><Bucket>b</Bucket></Logging>\
                  <Enabled>true</Enabled></DistributionConfig>";
    assert_eq!(
        config_enabled(config),
        Some(true),
        "case \"a configuration with several Enabled members\": expected the root's own, got \
         {:?}",
        config_enabled(config)
    );
    let disabled = with_root_member_text(config, "Enabled", "false")
        .expect("a configuration carrying the member");
    assert_eq!(
        config_enabled(&disabled),
        Some(false),
        "case \"the disabled configuration\": expected Enabled false, got {disabled}"
    );
    for kept in [
        "<Logging><Enabled>true</Enabled>",
        "<TrustedSigners><Enabled>false</Enabled>",
        "<CallerReference>r</CallerReference>",
    ] {
        assert!(
            disabled.contains(kept),
            "case \"the disabled configuration\": expected {kept} to be kept, got {disabled}"
        );
    }
    assert!(
        with_root_member_text(config, "Staging", "false").is_none(),
        "case \"a member the configuration does not carry\": expected no document"
    );
}
