use serde_json::Value;
use std::sync::OnceLock;

const BASE_URL: &str = "https://api.brawlstars.com/v1";
const TAG_CHARS: &str = "0289PYLQGRJCUV";

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

pub fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("failed to build HTTP client")
    })
}

/// "#2pp" / " 2PP " / "#2OO" -> "2PP" / "200" (letter O is never valid in a tag).
pub fn normalize_tag(tag: &str) -> String {
    tag.trim()
        .trim_start_matches('#')
        .to_uppercase()
        .replace('O', "0")
}

fn validate_tag(tag: &str) -> Result<(), String> {
    if tag.is_empty() {
        return Err("Enter a player tag.".into());
    }
    if let Some(bad) = tag.chars().find(|c| !TAG_CHARS.contains(*c)) {
        return Err(format!(
            "'{bad}' is not valid in a player tag. Tags only use these characters: {TAG_CHARS}"
        ));
    }
    Ok(())
}

pub async fn get(api_key: &str, path: &str) -> Result<Value, String> {
    if api_key.trim().is_empty() {
        return Err("No API key set. Add one in Settings.".into());
    }

    let res = client()
        .get(format!("{BASE_URL}{path}"))
        .bearer_auth(api_key.trim())
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = res.status();
    if status.is_success() {
        return res
            .json::<Value>()
            .await
            .map_err(|e| format!("Unexpected response from the API: {e}"));
    }

    let body = res.text().await.unwrap_or_default();
    Err(match status.as_u16() {
        403 => "The API rejected your key (403). Your public IP address probably changed or isn't on the key's allowed list. Update it at developer.brawlstars.com.".into(),
        404 => "Player not found. Double-check the tag.".into(),
        429 => "Too many requests (429). Wait a moment and try again.".into(),
        503 => "Brawl Stars is under maintenance (503). Try again later.".into(),
        code => format!("API error {code}: {body}"),
    })
}

pub async fn fetch_player(api_key: &str, tag: &str) -> Result<Value, String> {
    let tag = normalize_tag(tag);
    validate_tag(&tag)?;
    get(api_key, &format!("/players/%23{tag}")).await
}

/// Last 25 battles, newest first.
pub async fn fetch_battlelog(api_key: &str, tag: &str) -> Result<Value, String> {
    let tag = normalize_tag(tag);
    validate_tag(&tag)?;
    get(api_key, &format!("/players/%23{tag}/battlelog")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_tags() {
        assert_eq!(normalize_tag(" #8qgvry9up "), "8QGVRY9UP");
        assert_eq!(normalize_tag("#2OO"), "200");
    }

    #[test]
    fn validates_tags() {
        assert!(validate_tag("8QGVRY9UP").is_ok());
        assert!(validate_tag("").is_err());
        assert!(validate_tag("ABC").is_err()); // A, B are not tag characters
    }
}
