use serde::Deserialize;
use thiserror::Error;

pub const MANIFEST_URL: &str = "https://sylvan.apple.com/Aerials/2x/entries.json";
const FALLBACK_MANIFEST: &str = include_str!("../assets/entries.fallback.json");

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AerialAsset {
    pub id: String,
    #[serde(rename = "accessibilityLabel")]
    pub accessibility_label: String,
    #[serde(rename = "url-1080-SDR")]
    pub url_1080_sdr: String,
    #[serde(rename = "url-1080-HDR")]
    pub url_1080_hdr: String,
    #[serde(rename = "url-4K-SDR")]
    pub url_4k_sdr: String,
    #[serde(rename = "url-4K-HDR")]
    pub url_4k_hdr: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AerialManifestData {
    pub version: u32,
    #[serde(rename = "initialAssetCount")]
    pub initial_asset_count: u32,
    pub assets: Vec<AerialAsset>,
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("network request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("failed to parse manifest json: {0}")]
    Parse(#[from] serde_json::Error),
}

pub fn parse_manifest(body: &str) -> Result<AerialManifestData, ManifestError> {
    Ok(serde_json::from_str(body)?)
}

pub fn fallback_manifest() -> AerialManifestData {
    parse_manifest(FALLBACK_MANIFEST).expect("bundled fallback manifest must be valid JSON")
}

pub async fn fetch_manifest(client: &reqwest::Client) -> Result<AerialManifestData, ManifestError> {
    let body = client.get(MANIFEST_URL).send().await?.text().await?;
    parse_manifest(&body)
}

/// Fetches the live manifest, falling back to the bundled snapshot on any error.
pub async fn fetch_manifest_or_fallback(client: &reqwest::Client) -> AerialManifestData {
    match fetch_manifest(client).await {
        Ok(data) => data,
        Err(err) => {
            tracing::warn!("falling back to bundled Aerial manifest: {err}");
            fallback_manifest()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/entries.json");

    #[test]
    fn parses_real_schema_fixture() {
        let data = parse_manifest(FIXTURE).expect("fixture must parse");
        assert_eq!(data.version, 1);
        assert_eq!(data.initial_asset_count, 2);
        assert_eq!(data.assets.len(), 2);

        let la = &data.assets[0];
        assert_eq!(la.id, "829E69BA-BB53-4841-A138-4DF0C2A74236");
        assert_eq!(la.accessibility_label, "Los Angeles");
        assert_eq!(
            la.url_1080_sdr,
            "https://sylvan.apple.com/Aerials/2x/Videos/LA_A006_C008_2K_SDR_HEVC.mov"
        );
        assert_eq!(
            la.url_4k_hdr,
            "https://sylvan.apple.com/Aerials/2x/Videos/LA_A006_C008_4K_HDR_HEVC.mov"
        );
    }

    #[test]
    fn bundled_fallback_manifest_is_valid() {
        let data = fallback_manifest();
        assert!(!data.assets.is_empty());
    }
}
