#![forbid(unsafe_code)]
//! Normalizes rejected HTTP responses and transport errors into typed failures.

use krabb_core::failure::Kind;
use reqwest::header::{HeaderValue, RETRY_AFTER};

const BODY_LIMIT: usize = 4096;
const BODY_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
const REASON_LIMIT: usize = 200;

/// Seconds a `Retry-After` header asks to wait, in delay or HTTP-date form.
pub fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    krabb_date::retry_hint(headers.get(RETRY_AFTER).map(HeaderValue::as_bytes)).flatten()
}

/// Classifies a rejected response from its status and `Retry-After` header. A malformed
/// or missing resource also carries the provider's JSON error message, clipped and
/// sanitized; other bodies are never echoed.
pub async fn rejected(mut response: reqwest::Response, provider: &str) -> krabb_core::Error {
    let error = status(response.status(), response.headers().get(RETRY_AFTER), provider);

    let krabb_core::Error::Failed { failure, message } = error else { return error };

    if !matches!(
        krabb_core::failure::kind(response.status().as_u16()),
        Kind::Invalid | Kind::Missing
    ) {
        return failure.error(message);
    }

    let mut body = Vec::new();

    let deadline = tokio::time::Instant::now() + BODY_WAIT;

    while body.len() < BODY_LIMIT {
        match tokio::time::timeout_at(deadline, response.chunk()).await.unwrap_or(Ok(None)) {
            Ok(Some(chunk)) => {
                let room = BODY_LIMIT - body.len();

                body.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }

            _ => break,
        }
    }

    match reason(&body) {
        Some(reason) => failure.error(format!("{message} {reason}")),
        None => failure.error(message),
    }
}

fn reason(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let error = &value["error"];
    let text = error["message"].as_str().or(error.as_str()).or(value["message"].as_str())?;

    let line = text.lines().next().unwrap_or_default();
    let mut visible = line.chars().filter(|character| !character.is_control());

    let mut reason: String = visible.by_ref().take(REASON_LIMIT).collect::<String>().trim().into();

    if reason.is_empty() {
        return None;
    }

    if visible.next().is_some() {
        reason.push_str("...");
    }

    Some(reason)
}

pub fn status(
    status: reqwest::StatusCode,
    retry: Option<&HeaderValue>,
    provider: &str,
) -> krabb_core::Error {
    krabb_core::failure::status(
        status.as_u16(),
        krabb_date::retry_hint(retry.map(HeaderValue::as_bytes)),
        provider,
    )
}

pub fn transport(error: &reqwest::Error, provider: &str) -> krabb_core::Error {
    use krabb_core::failure::Transport;

    let cause = if error.is_timeout() {
        Transport::TimedOut
    } else if error.is_connect() {
        Transport::Unreachable
    } else if error.is_body() || error.is_decode() || error.is_request() && !error.is_builder() {
        Transport::Interrupted
    } else {
        Transport::Unsent
    };

    krabb_core::failure::transport(cause, provider)
}

/// Sends a read-only request and succeeds when the provider accepts its credential.
pub async fn accepted(request: reqwest::RequestBuilder, provider: &str) -> krabb_core::Result<()> {
    let response = request.send().await.map_err(|error| transport(&error, provider))?;

    if !response.status().is_success() {
        return Err(rejected(response, provider).await);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use krabb_core::failure::Failure;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn failure(error: krabb_core::Error) -> Failure {
        match error {
            krabb_core::Error::Failed { failure, .. } => failure,
            error => panic!("Unexpected failure: {error}"),
        }
    }

    #[tokio::test]
    async fn a_stalled_error_body_does_not_hold_the_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = socket.read(&mut request).await.unwrap();

            socket
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 100\r\n\r\n{\"error\"")
                .await
                .unwrap();

            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });

        let response = reqwest::Client::new().get(&address).send().await.unwrap();
        let started = std::time::Instant::now();
        let error = rejected(response, "Provider").await;

        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(failure(error).kind, Kind::Invalid);
    }

    async fn serve(reply: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = socket.read(&mut request).await.unwrap();

            if !reply.is_empty() {
                socket.write_all(reply.as_bytes()).await.unwrap();
            }

            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        });

        address
    }

    #[tokio::test]
    async fn accepted_succeeds_only_for_an_accepted_credential() {
        let client = reqwest::Client::new();
        let allowed = serve("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").await;

        accepted(client.get(&allowed), "Provider").await.unwrap();

        let denied = serve("HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\n\r\n{}").await;
        let error = accepted(client.get(&denied), "Provider").await.unwrap_err();

        assert_eq!(failure(error).kind, Kind::Denied);

        let error = accepted(client.get("http://127.0.0.1:1"), "Provider").await.unwrap_err();

        assert!(error.to_string().contains("Provider"), "{error}");
    }

    #[tokio::test]
    async fn rejected_responses_and_transport_errors_become_typed_failures() {
        let client = reqwest::Client::new();

        let throttled = serve(
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7\r\nContent-Length: 13\r\n\r\nprivate \
                quota",
        )
        .await;

        let response = client.get(&throttled).send().await.unwrap();
        let error = rejected(response, "Provider").await;

        assert_eq!(error.to_string(), "Provider rejected the request with HTTP 429.");
        assert_eq!(failure(error), Failure { kind: Kind::Throttled, retry_after_seconds: Some(7) });

        let unclear = serve(
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: soon\r\nContent-Length: 45\r\n\r\n\
             {\"error\":{\"message\":\"Quota for org-private\"}}",
        )
        .await;

        let response = client.get(&unclear).send().await.unwrap();
        let error = rejected(response, "Provider").await;

        assert!(!error.to_string().contains("org-private"), "{error}");
        assert_eq!(failure(error).kind, Kind::Invalid);

        let denied = serve(
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: 46\r\n\r\n\
             {\"error\":{\"message\":\"Invalid key sk-private\"}}",
        )
        .await;

        let response = client.get(&denied).send().await.unwrap();
        let error = rejected(response, "Provider").await;

        assert_eq!(error.to_string(), "Provider rejected the request with HTTP 401.");
        assert_eq!(failure(error).kind, Kind::Denied);

        let invalid = serve(
            "HTTP/1.1 400 Bad Request\r\nContent-Length: 68\r\n\r\n\
             {\"error\":{\"message\":\"Unknown model\\u0007 fixture.\\nprivate detail\"}}",
        )
        .await;

        let response = client.get(&invalid).send().await.unwrap();
        let error = rejected(response, "Provider").await;

        assert_eq!(
            error.to_string(),
            "Provider rejected the request with HTTP 400. Unknown model fixture."
        );

        assert_eq!(failure(error).kind, Kind::Invalid);

        let unstructured =
            serve("HTTP/1.1 404 Not Found\r\nContent-Length: 14\r\n\r\nprivate detail").await;

        let response = client.get(&unstructured).send().await.unwrap();

        assert_eq!(
            rejected(response, "Provider").await.to_string(),
            "Provider rejected the request with HTTP 404."
        );

        let silent = serve("").await;

        let error = client
            .get(&silent)
            .timeout(std::time::Duration::from_millis(100))
            .send()
            .await
            .unwrap_err();

        assert_eq!(failure(transport(&error, "Provider")).kind, Kind::TimedOut);

        let closed = tokio::net::TcpSocket::new_v4().unwrap();

        closed.bind("127.0.0.1:0".parse().unwrap()).unwrap();

        let address = format!("http://{}", closed.local_addr().unwrap());
        let error = client.get(&address).send().await.unwrap_err();

        drop(closed);

        assert!(error.is_connect(), "{error:?}");
        assert_eq!(failure(transport(&error, "Provider")).kind, Kind::Transient);
        assert!(transport(&error, "Provider").to_string().contains("could not be reached"));

        let reset = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", reset.local_addr().unwrap());

        tokio::spawn(async move {
            let (mut socket, _) = reset.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = socket.read(&mut request).await;
        });

        let error = client.get(&address).send().await.unwrap_err();

        assert!(!error.is_connect(), "{error:?}");
        assert_eq!(failure(transport(&error, "Provider")).kind, Kind::Transient);
        assert!(transport(&error, "Provider").to_string().contains("interrupted"));

        let garbled = serve("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n{x}").await;
        let response = client.get(&garbled).send().await.unwrap();
        let error = response.json::<serde_json::Value>().await.unwrap_err();

        assert!(error.is_decode(), "{error:?}");
        assert_eq!(failure(transport(&error, "Provider")).kind, Kind::Transient);

        let error = client.get("https://[invalid").send().await.unwrap_err();

        assert_eq!(failure(transport(&error, "Provider")).kind, Kind::Invalid);
    }

    #[test]
    fn retry_after_reads_delays_and_http_dates() {
        let mut headers = reqwest::header::HeaderMap::new();

        assert_eq!(retry_after(&headers), None);

        headers.insert(RETRY_AFTER, HeaderValue::from_static("7"));

        assert_eq!(retry_after(&headers), Some(7));

        headers.insert(RETRY_AFTER, HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"));

        assert_eq!(retry_after(&headers), Some(0));

        headers.insert(RETRY_AFTER, HeaderValue::from_static("soon"));

        assert_eq!(retry_after(&headers), None);
    }

    #[test]
    fn a_long_reason_is_clipped_to_its_first_line() {
        let long = format!("{{\"error\":\"{}\\nsecond\"}}", "x".repeat(300));

        assert_eq!(reason(long.as_bytes()), Some(format!("{}...", "x".repeat(200))));

        let exact = format!(r#"{{"message":"\u0007{}\u0007"}}"#, "x".repeat(200));

        assert_eq!(reason(exact.as_bytes()), Some("x".repeat(200)));
        assert_eq!(reason(br#"{"message":"  "}"#), None);
        assert_eq!(reason(b"not json"), None);
    }
}
