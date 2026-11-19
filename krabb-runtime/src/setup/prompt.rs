//! Manifest metadata that explains and validates each setup field.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const LABEL: usize = 64;
const HELP: usize = 320;
const EXAMPLE: usize = 128;
const URL: usize = 512;
const ACCEPT: usize = 16;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Prompt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Kind::is_text")]
    pub kind: Kind,
    /// Literal values accepted in addition to `kind`, such as `auto`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accept: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    #[default]
    Text,
    Integer,
    Boolean,
    Url,
    Address,
}

impl Kind {
    fn is_text(&self) -> bool {
        *self == Self::Text
    }

    fn valid(self, value: &str) -> bool {
        match self {
            Self::Text => true,
            Self::Integer => value.parse::<u64>().is_ok(),
            Self::Boolean => matches!(value, "true" | "false"),

            Self::Url => ["https://", "http://"].iter().any(|scheme| {
                value.strip_prefix(scheme).is_some_and(|rest| {
                    !rest.is_empty()
                        && !rest.starts_with('/')
                        && !rest.chars().any(char::is_whitespace)
                })
            }),

            Self::Address => value.rsplit_once(':').is_some_and(|(host, port)| {
                !host.is_empty()
                    && !host.chars().any(char::is_whitespace)
                    && port.parse::<u16>().is_ok_and(|port| port > 0)
            }),
        }
    }

    fn expected(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Integer => "a whole number",
            Self::Boolean => "true or false",
            Self::Url => "an http:// or https:// URL",
            Self::Address => "a host:port address",
        }
    }
}

impl Prompt {
    /// Checks a typed value without ever quoting it, since it can be a secret.
    pub(crate) fn check(&self, value: &str) -> Result<(), String> {
        if self.accept.iter().any(|accepted| accepted == value) || self.kind.valid(value) {
            return Ok(());
        }

        let mut expected = self.kind.expected().to_owned();

        if !self.accept.is_empty() {
            expected = format!("{expected} or {}", self.accept.join(", "));
        }

        Err(format!("Enter {expected}."))
    }
}

/// Validates manifest metadata against the declared names and setting defaults.
pub(crate) fn validate(
    prompts: &BTreeMap<String, Prompt>,
    names: &[String],
    defaults: &[(String, Option<String>)],
) -> Result<(), String> {
    let invalid = |field: &str, name: &str| format!("Setup {field} for {name} is invalid.");

    for (name, prompt) in prompts {
        if !names.contains(name) {
            return Err(format!("Setup metadata names an undeclared field: {name}."));
        }

        for (field, value, limit) in [
            ("label", &prompt.label, LABEL),
            ("help", &prompt.help, HELP),
            ("example", &prompt.example, EXAMPLE),
            ("url", &prompt.url, URL),
        ] {
            if let Some(value) = value
                && (super::validate(value).is_err() || value.len() > limit)
            {
                return Err(invalid(field, name));
            }
        }

        if prompt.url.as_deref().is_some_and(|url| !url.starts_with("https://")) {
            return Err(invalid("url", name));
        }

        if prompt.accept.len() > ACCEPT
            || prompt.accept.iter().any(|value| super::validate(value).is_err() || value == "-")
        {
            return Err(invalid("accept", name));
        }

        let default = defaults.iter().find(|(setting, _)| setting == name);

        if let Some((_, Some(default))) = default
            && prompt.check(default).is_err()
        {
            return Err(invalid("default", name));
        }
    }

    Ok(())
}
