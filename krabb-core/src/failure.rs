use serde::{Deserialize, Serialize};

pub const DELAY_LIMIT: u64 = 7 * 24 * 60 * 60;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Transient,
    TimedOut,
    Throttled,
    Exhausted,
    Invalid,
    Denied,
    Missing,
    Cancelled,
    Uncertain,
    Unknown,
}

impl Kind {
    pub fn retryable(self) -> bool {
        matches!(self, Self::Transient | Self::TimedOut | Self::Throttled)
    }

    /// A broken stream is transient until text reaches the user; after that a replay would
    /// repeat or contradict the shown text, so the outcome is uncertain.
    pub fn interrupted(emitted: usize) -> Self {
        if emitted == 0 { Self::Transient } else { Self::Uncertain }
    }

    /// Whether the provider refused the request before generating anything.
    pub fn refused(self) -> bool {
        matches!(
            self,
            Self::Throttled | Self::Denied | Self::Exhausted | Self::Missing | Self::Invalid
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
}

impl Failure {
    pub fn new(kind: Kind) -> Self {
        Self { kind, retry_after_seconds: None }
    }

    pub fn validate(&self) -> crate::Result<()> {
        if self
            .retry_after_seconds
            .is_some_and(|delay| !self.kind.retryable() || delay > DELAY_LIMIT)
        {
            return Err(crate::Error::Protocol("Invalid failure retry allowance.".into()));
        }

        Ok(())
    }

    pub fn error(self, message: impl Into<String>) -> crate::Error {
        crate::Error::Failed { message: message.into(), failure: self }
    }
}

/// The failure kind an HTTP status reports, before any retry hint is considered.
pub fn kind(code: u16) -> Kind {
    match code {
        401 | 403 => Kind::Denied,
        402 => Kind::Exhausted,
        429 => Kind::Throttled,
        501 | 505 => Kind::Invalid,
        408 | 425 | 500..=599 => Kind::Transient,
        404 => Kind::Missing,
        _ => Kind::Invalid,
    }
}

/// Classifies a rejected HTTP status without reading the response body.
///
/// `retry` is the parsed `Retry-After` guidance: `None` when absent and
/// `Some(None)` when present but unusable. Unusable or excessive guidance on a
/// retryable status requires review instead of an automatic retry.
pub fn status(code: u16, retry: Option<Option<u64>>, provider: &str) -> crate::Error {
    let kind = kind(code);

    let retry_after_seconds = match retry.filter(|_| kind.retryable()) {
        Some(Some(delay)) if delay <= DELAY_LIMIT => Some(delay),

        Some(_) => {
            return Failure::new(Kind::Invalid).error(format!(
                "{provider} returned an unsupported retry time. Review it before resuming."
            ));
        }

        None => None,
    };

    Failure { kind, retry_after_seconds }
        .error(format!("{provider} rejected the request with HTTP {code}."))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transport {
    TimedOut,
    Unreachable,
    Interrupted,
    Unsent,
}

/// Classifies a transport failure from the facts an HTTP client reports.
pub fn transport(cause: Transport, provider: &str) -> crate::Error {
    let (kind, reason) = match cause {
        Transport::TimedOut => (Kind::TimedOut, "the request timed out"),
        Transport::Unreachable => (Kind::Transient, "the provider could not be reached"),
        Transport::Interrupted => (Kind::Transient, "the connection was interrupted"),
        Transport::Unsent => (Kind::Invalid, "the request could not be completed"),
    };

    Failure::new(kind).error(format!("{provider} transport failed: {reason}."))
}

/// Frames a plugin error for the host. A decode error may follow a finished generation, so it
/// stays unknown rather than claiming the provider refused before spending anything.
pub fn response(id: u64, error: crate::Error) -> crate::types::Response {
    let failure = match &error {
        crate::Error::Failed { failure, .. } if failure.validate().is_ok() => failure.clone(),
        crate::Error::Denied(_) => Failure::new(Kind::Denied),
        crate::Error::Limit(_) => Failure::new(Kind::Exhausted),

        crate::Error::Protocol(_) | crate::Error::Handshake(_) => Failure::new(Kind::Invalid),

        _ => Failure::new(Kind::Unknown),
    };

    let mut response = crate::types::Response::fail(id, -32000, error.to_string());

    response.error.as_mut().unwrap().data = Some(serde_json::json!({"failure":failure}));
    response
}

const MESSAGE_LIMIT: usize = 4096;

/// Normalizes a plugin error: long messages are clipped and undeclared or malformed
/// metadata becomes a final unknown failure, so neither is retried automatically.
pub fn decode(error: crate::types::RpcError) -> crate::Error {
    let mut message = error.message.trim().to_owned();

    if message.is_empty() {
        message = "The plugin reported an error without a description.".into();
    }

    if message.len() > MESSAGE_LIMIT {
        let mut end = MESSAGE_LIMIT - 3;

        while !message.is_char_boundary(end) {
            end -= 1;
        }

        message.truncate(end);
        message.push_str("...");
    }

    let failure = error
        .data
        .as_ref()
        .and_then(|data| data.get("failure"))
        .and_then(|value| serde_json::from_value::<Failure>(value.clone()).ok())
        .filter(|failure| failure.validate().is_ok())
        .unwrap_or(Failure::new(Kind::Unknown));

    failure.error(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_stream_broken_after_shown_text_is_uncertain() {
        assert_eq!(Kind::interrupted(0), Kind::Transient);
        assert_eq!(Kind::interrupted(1), Kind::Uncertain);
        assert!(!Kind::interrupted(1).retryable());
        assert!(Kind::Invalid.refused());
        assert!(!Kind::Uncertain.refused());
    }

    #[test]
    fn http_statuses_and_transport_facts_map_to_typed_recovery() {
        let kind = |error: crate::Error| match error {
            crate::Error::Failed { failure, .. } => (failure.kind, failure.retry_after_seconds),
            error => panic!("Unexpected failure: {error}"),
        };

        for (code, retry, expected) in [
            (429, Some(Some(2)), (Kind::Throttled, Some(2))),
            (503, Some(Some(0)), (Kind::Transient, Some(0))),
            (529, None, (Kind::Transient, None)),
            (500, None, (Kind::Transient, None)),
            (522, None, (Kind::Transient, None)),
            (599, None, (Kind::Transient, None)),
            (501, None, (Kind::Invalid, None)),
            (505, None, (Kind::Invalid, None)),
            (401, Some(None), (Kind::Denied, None)),
            (403, Some(Some(5)), (Kind::Denied, None)),
            (402, None, (Kind::Exhausted, None)),
            (404, None, (Kind::Missing, None)),
            (400, None, (Kind::Invalid, None)),
            (429, Some(None), (Kind::Invalid, None)),
            (503, Some(Some(DELAY_LIMIT + 1)), (Kind::Invalid, None)),
        ] {
            assert_eq!(kind(status(code, retry, "Provider")), expected, "{code}");
        }

        assert_eq!(
            status(429, None, "Provider").to_string(),
            "Provider rejected the request with HTTP 429."
        );

        assert_eq!(kind(transport(Transport::TimedOut, "Provider")), (Kind::TimedOut, None));
        assert_eq!(kind(transport(Transport::Unreachable, "Provider")), (Kind::Transient, None));
        assert_eq!(kind(transport(Transport::Interrupted, "Provider")), (Kind::Transient, None));
        assert_eq!(kind(transport(Transport::Unsent, "Provider")), (Kind::Invalid, None));
    }

    #[test]
    fn typed_failures_preserve_hints_and_legacy_frames_stay_readable() {
        let failure = Failure { kind: Kind::Throttled, retry_after_seconds: Some(120) };
        let response = response(7, failure.clone().error("Please wait before retrying."));
        let bytes = serde_json::to_vec(&response).unwrap();
        let parsed: crate::types::Response = serde_json::from_slice(&bytes).unwrap();

        assert!(parsed.valid());

        match decode(parsed.error.unwrap()) {
            crate::Error::Failed { failure: actual, message } => {
                assert_eq!(actual, failure);
                assert_eq!(message, "Please wait before retrying.");
            }

            error => panic!("Unexpected failure: {error}"),
        }

        let undecoded = crate::Error::from(serde_json::from_str::<u8>("x").unwrap_err());

        assert!(matches!(
            decode(super::response(9, undecoded).error.unwrap()),
            crate::Error::Failed { failure, .. } if failure.kind == Kind::Unknown
        ));

        let legacy = crate::types::Response::fail(8, -32000, "Legacy provider error.");

        assert!(matches!(
            decode(legacy.error.unwrap()),
            crate::Error::Failed { failure: Failure { kind: Kind::Unknown, .. }, .. }
        ));

        let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(wire["error"]["message"], "Please wait before retrying.");
        assert_eq!(wire["error"]["data"]["failure"]["kind"], "throttled");
    }

    #[test]
    fn explicit_invalid_failure_metadata_never_becomes_an_automatic_retry() {
        for metadata in [
            json!(null),
            json!({"kind":"future"}),
            json!({"kind":"transient","retry_after_seconds":-1}),
            json!({"kind":"transient","retry_after_seconds":DELAY_LIMIT+1}),
            json!({"kind":"denied","retry_after_seconds":1}),
            json!({"kind":"transient","unexpected":true}),
        ] {
            let mut error =
                crate::types::Response::fail(1, -32000, "Provider error.").error.unwrap();
            error.data = Some(json!({"failure":metadata}));

            assert!(matches!(
                decode(error),
                crate::Error::Failed { failure: Failure { kind: Kind::Unknown, .. }, .. }
            ));
        }

        let mut long = crate::types::Response::fail(1, -32000, "é".repeat(4000)).error.unwrap();

        long.data = Some(json!({"failure":{"kind":"throttled","retry_after_seconds":5}}));

        match decode(long) {
            crate::Error::Failed { failure, message } => {
                assert_eq!(failure.kind, Kind::Throttled);
                assert_eq!(failure.retry_after_seconds, Some(5));
                assert!(message.len() <= MESSAGE_LIMIT);
                assert!(message.ends_with("..."));
            }

            error => panic!("Unexpected failure: {error}"),
        }

        let blank = crate::types::Response::fail(1, -32000, " ").error.unwrap();

        assert!(decode(blank).to_string().contains("without a description"));

        let failure = Failure { kind: Kind::Transient, retry_after_seconds: Some(DELAY_LIMIT + 1) };

        let response = response(1, failure.error("Invalid provider hint."));

        assert_eq!(response.error.unwrap().data.unwrap()["failure"]["kind"], "unknown");
    }
}
