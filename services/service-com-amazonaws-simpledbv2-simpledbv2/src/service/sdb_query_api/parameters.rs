// SPDX-License-Identifier: CC0-1.0

//! The parameters of one Amazon SimpleDB 2009-04-15 request, read off the form it
//! arrived in.
//!
//! The AWS Query API carries its parameters as `application/x-www-form-urlencoded`
//! fields: in the BODY of a POST and in the QUERY STRING of a GET, which are the two
//! forms the API accepts. The decoding itself is the framework's
//! (`aws_smithy_schema_server::parse_form_urlencoded`, the same reader the query
//! protocols route on), so nothing here parses the wire format a second time: this
//! module only says what the decoded fields MEAN.
//!
//! A LIST parameter is indexed — `Attribute.0.Name`, `Attribute.1.Value`,
//! `AttributeName.2` — and the API documents the index as "N", with no requirement that
//! the indexes be contiguous or start anywhere in particular. So an indexed group is read
//! by its index whatever the numbers are, in index order, and a parameter whose shape
//! does not fit the group is simply not part of it.

use std::collections::BTreeMap;

use aws_smithy_runtime_api::http::Request as RtRequest;

/// The decoded fields of one request, in the order they arrived.
#[derive(Debug, Default)]
pub struct QueryParameters {
    fields: Vec<(String, String)>,
}

impl QueryParameters {
    /// The parameters of `req`: its form-urlencoded body when it carries one, else its
    /// query string.
    ///
    /// The router buffers every request before any seam that reads one, so the body is
    /// available here as bytes; an empty body is a GET-style request whose parameters are
    /// in the URI.
    pub fn of(req: &RtRequest) -> Self {
        let body = req.body().bytes().unwrap_or_default();
        if !body.is_empty() {
            return Self {
                fields: ::aws_smithy_schema_server::parse_form_urlencoded(body),
            };
        }
        let query = req
            .uri()
            .split_once('?')
            .map_or("", |(_, query)| query)
            .split('#')
            .next()
            .unwrap_or_default();
        Self {
            fields: ::aws_smithy_schema_server::parse_form_urlencoded(query.as_bytes()),
        }
    }

    /// The value of `name`, or `None` when the request carries no such parameter.
    ///
    /// The FIRST occurrence decides: a parameter sent twice is one parameter with one
    /// value, and reading the first is what makes a repeat unable to change a decision
    /// already taken on it.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.as_str())
    }

    /// The members of the indexed group `prefix`, in index order: for `Attribute`, every
    /// `Attribute.<N>.<Member>` field, grouped by `<N>`, each group paired with its own
    /// index so a refusal can name the parameter the request actually sent.
    ///
    /// The index is read as a number, so `Attribute.10` sorts after `Attribute.9` rather
    /// than between `Attribute.1` and `Attribute.2`. A member sent twice under one index
    /// keeps its first value, for the reason [`Self::value`] gives.
    pub fn indexed_members(&self, prefix: &str) -> Vec<(u32, BTreeMap<&str, &str>)> {
        let mut groups: BTreeMap<u32, BTreeMap<&str, &str>> = BTreeMap::new();
        for (field, value) in &self.fields {
            let mut parts = field.split('.');
            let (Some(name), Some(index), Some(member), None) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Ok(index) = index.parse::<u32>() else {
                continue;
            };
            if name != prefix {
                continue;
            }
            groups
                .entry(index)
                .or_default()
                .entry(member)
                .or_insert(value.as_str());
        }
        groups.into_iter().collect()
    }

    /// The values of the indexed group `prefix` whose members are the values themselves:
    /// for `AttributeName`, every `AttributeName.<N>` field, in index order.
    pub fn indexed_values(&self, prefix: &str) -> Vec<&str> {
        let mut values: BTreeMap<u32, &str> = BTreeMap::new();
        for (field, value) in &self.fields {
            let mut parts = field.split('.');
            let (Some(name), Some(index), None) = (parts.next(), parts.next(), parts.next()) else {
                continue;
            };
            let Ok(index) = index.parse::<u32>() else {
                continue;
            };
            if name != prefix {
                continue;
            }
            values.entry(index).or_insert(value.as_str());
        }
        values.into_values().collect()
    }
}

/// Whether `value` is the Query API's spelling of true.
///
/// The API documents a boolean parameter as `true` or `false`; the comparison is
/// case-insensitive so a client that sends `True` is not read as having sent false.
/// Anything else is not true, which is the parameter's own default for every boolean of
/// this API (`Replace`, `Expected.Exists`, `ConsistentRead`).
pub fn is_true(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("true")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_types::body::SdkBody;

    fn posted(body: &'static str) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::from(body));
        req.set_uri("/").expect("a valid uri");
        req
    }

    #[test]
    fn a_posted_form_is_read_as_the_parameters() {
        let params = posted("Action=CreateDomain&DomainName=my-domain&Version=2009-04-15");
        let params = QueryParameters::of(&params);
        assert_eq!(
            params.value("Action"),
            Some("CreateDomain"),
            "case a posted form: Action must be read from the body"
        );
        assert_eq!(
            params.value("DomainName"),
            Some("my-domain"),
            "case a posted form: DomainName must be read from the body"
        );
        assert_eq!(
            params.value("ItemName"),
            None,
            "case a posted form: a parameter the form does not carry is absent"
        );
    }

    #[test]
    fn a_query_string_is_read_when_there_is_no_body() {
        let mut req = RtRequest::new(SdkBody::empty());
        req.set_uri("/?Action=DeleteDomain&DomainName=gone")
            .expect("a valid uri");
        let params = QueryParameters::of(&req);
        assert_eq!(
            params.value("Action"),
            Some("DeleteDomain"),
            "case a GET-style request: Action must be read from the query string"
        );
    }

    #[test]
    fn indexed_members_are_grouped_by_their_index_in_numeric_order() {
        let params = QueryParameters::of(&posted(
            "Attribute.10.Name=late&Attribute.10.Value=ten&Attribute.2.Name=early\
             &Attribute.2.Value=two&Attribute.2.Replace=true&Nonsense.x.Name=skipped",
        ));
        let groups = params.indexed_members("Attribute");
        let names: Vec<(u32, Option<&&str>)> = groups
            .iter()
            .map(|(index, group)| (*index, group.get("Name")))
            .collect();
        assert_eq!(
            names,
            vec![(2, Some(&"early")), (10, Some(&"late"))],
            "case indexes 2 and 10: groups must be ordered numerically and keep their \
             index, got {groups:?}"
        );
        assert_eq!(
            groups.first().and_then(|(_, group)| group.get("Replace")),
            Some(&"true"),
            "case index 2: every member of one index belongs to one group, got {groups:?}"
        );
    }

    #[test]
    fn indexed_values_read_a_list_whose_members_are_the_values() {
        let params = QueryParameters::of(&posted(
            "AttributeName.1=colour&AttributeName.0=size&AttributeName.bad=skipped",
        ));
        assert_eq!(
            params.indexed_values("AttributeName"),
            vec!["size", "colour"],
            "case AttributeName.0 and .1: the values must be read in index order"
        );
    }

    #[test]
    fn the_boolean_spelling_is_case_insensitive_and_nothing_else_is_true() {
        for case in ["true", "TRUE", " True "] {
            assert!(is_true(case), "case {case:?}: must read as true");
        }
        for case in ["false", "", "1", "yes"] {
            assert!(!is_true(case), "case {case:?}: must not read as true");
        }
    }
}
