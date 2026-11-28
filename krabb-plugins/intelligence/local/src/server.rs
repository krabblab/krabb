//! Finds and reads an OpenAI-compatible inference server on this machine or its network.

use std::net::IpAddr;
use std::time::Duration;

use krabb_core::{Error, Result};
use serde_json::Value;

use super::PROVIDER;

/// Default addresses of well-known servers, tried in order when the URL is `auto`.
pub(crate) const KNOWN: &[(&str, &str)] = &[
    ("vLLM", "http://127.0.0.1:8000/v1"),
    ("llama.cpp", "http://127.0.0.1:8080/v1"),
    ("LM Studio", "http://127.0.0.1:1234/v1"),
    ("Jan", "http://127.0.0.1:1337/v1"),
    ("KoboldCpp", "http://127.0.0.1:5001/v1"),
    ("SGLang", "http://127.0.0.1:30000/v1"),
];

const PROBE: Duration = Duration::from_secs(2);
const MODELS: usize = 256;
const FALLBACK_CONTEXT: u64 = 4096;
const NAMES: &[&str] = &["local", "localhost", "lan", "internal", "home.arpa"];

/// Where requests go: the configured server, or the one found among the usual addresses. A
/// found server is remembered until a request to it fails, and never receives the API key,
/// which only a configured address gets.
pub(crate) struct Server {
    configured: String,
    candidates: Vec<(String, String)>,
    found: std::sync::Mutex<Option<String>>,
}

impl Server {
    pub(crate) fn new(configured: &str, candidates: &[(&str, &str)]) -> Self {
        let candidates =
            candidates.iter().map(|(name, url)| ((*name).to_owned(), (*url).to_owned())).collect();

        Self { configured: configured.trim().into(), candidates, found: Default::default() }
    }

    pub(crate) fn automatic(&self) -> bool {
        matches!(self.configured.as_str(), "" | "auto")
    }

    /// The base URL, and the KEY to send there: none for a found server.
    pub(crate) async fn base<'a>(
        &self,
        client: &reqwest::Client,
        key: Option<&'a str>,
    ) -> Result<(String, Option<&'a str>)> {
        if !self.automatic() {
            return Ok((validate(&self.configured)?, key));
        }

        let known = self.found.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();

        if let Some(base) = known {
            return Ok((base, None));
        }

        let base = find(client, &self.candidates).await?;

        *self.found.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(base.clone());

        Ok((base, None))
    }

    /// Forgets a found server after a failed request, so the next one looks again.
    pub(crate) fn forget(&self) {
        *self.found.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

async fn find(client: &reqwest::Client, candidates: &[(String, String)]) -> Result<String> {
    for (_, url) in candidates {
        if models_within(client, url, None, PROBE).await.is_ok() {
            return Ok(url.clone());
        }
    }

    let names: Vec<&str> = candidates.iter().map(|(name, _)| name.as_str()).collect();

    Err(Error::Denied(format!(
        "No local inference server answered on the usual ports ({}).\nStart one, or set its \
         address with `krabb local setup`.",
        names.join(", ")
    )))
}

/// Checks URL and returns it without a trailing slash. Plain HTTP is allowed only for loopback,
/// private, and local-network hosts, so a key or a conversation never crosses the internet in
/// clear text.
pub(crate) fn validate(url: &str) -> Result<String> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| Error::Denied("The local server address is not a valid URL.".into()))?;

    let plain = parsed.scheme() == "http";

    if !(parsed.scheme() == "https" || plain && parsed.host_str().is_some_and(nearby))
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(Error::Denied(
            "The local server address must use HTTPS outside this machine and its local network."
                .into(),
        ));
    }

    Ok(url.trim_end_matches('/').into())
}

/// Whether HOST is this machine or a private or local-network address.
fn nearby(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');

    if let Ok(address) = bare.parse::<IpAddr>() {
        return match address {
            IpAddr::V4(address) => {
                let [first, second, ..] = address.octets();

                address.is_loopback()
                    || address.is_private()
                    || address.is_link_local()
                    || first == 100 && (64..128).contains(&second)
            }

            IpAddr::V6(address) => {
                let segment = address.segments()[0];

                address.is_loopback() || segment & 0xfe00 == 0xfc00 || segment & 0xffc0 == 0xfe80
            }
        };
    }

    let host = host.trim_end_matches('.').to_ascii_lowercase();

    !host.contains('.')
        || NAMES.iter().any(|name| host == *name || host.ends_with(&format!(".{name}")))
}

/// The models BASE serves, as its `/models` entries.
pub(crate) async fn models(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
) -> Result<Vec<Value>> {
    models_within(client, base, key, Duration::from_secs(30)).await
}

async fn models_within(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    wait: Duration,
) -> Result<Vec<Value>> {
    let mut request = client.get(format!("{base}/models")).timeout(wait);

    if let Some(key) = key {
        request = request.bearer_auth(key);
    }

    let response = request.send().await.map_err(|error| krabb_http::transport(&error, PROVIDER))?;

    if !response.status().is_success() {
        return Err(krabb_http::rejected(response, PROVIDER).await);
    }

    let bytes = krabb_openai::chat::collect(PROVIDER, response.bytes_stream()).await?;

    let body: Value = serde_json::from_slice(&bytes)
        .map_err(|_| Error::Denied(format!("{PROVIDER} returned an invalid model list.")))?;

    let Some(models) = body["data"].as_array() else {
        return Err(Error::Denied(format!("{PROVIDER} returned an invalid model list.")));
    };

    Ok(models.iter().filter(|model| model["id"].is_string()).take(MODELS).cloned().collect())
}

/// MODEL, or the server's first model when MODEL leaves the choice open.
pub(crate) fn resolve(model: &str, models: &[Value]) -> Result<String> {
    if !matches!(model.trim(), "" | "default" | "unset") {
        return Ok(model.into());
    }

    models.first().and_then(|model| model["id"].as_str()).map(str::to_owned).ok_or_else(|| {
        Error::Denied(format!("{PROVIDER} has no model loaded.\nLoad a model in the server first."))
    })
}

/// The context length MODEL's entry reports, under the names vLLM, llama.cpp, LM Studio, and
/// others use, or a small default when the server does not say.
pub(crate) fn context(entry: Option<&Value>) -> u64 {
    let Some(entry) = entry else { return FALLBACK_CONTEXT };

    let reported = [
        &entry["max_model_len"],
        &entry["context_length"],
        &entry["max_context_length"],
        &entry["meta"]["n_ctx"],
    ];

    reported
        .into_iter()
        .filter_map(Value::as_u64)
        .find(|tokens| (2..=u64::from(u32::MAX)).contains(tokens))
        .unwrap_or(FALLBACK_CONTEXT)
}

/// The model list the `local models` command prints, in the `  id - name` form setup reads.
pub(crate) fn listing(base: &str, models: &[Value]) -> String {
    let lines: Vec<String> = models
        .iter()
        .filter_map(|model| model["id"].as_str())
        .filter(|id| !id.chars().any(char::is_control))
        .map(|id| format!("  {id} - {id}"))
        .collect();

    format!("Local models at {base}\n{}", lines.join("\n"))
}
