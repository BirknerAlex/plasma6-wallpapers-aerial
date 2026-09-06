use serde::Deserialize;
use thiserror::Error;

pub const MANIFEST_URL: &str = "https://sylvan.apple.com/Aerials/2x/entries.json";

/// Apple deprecated the flat `MANIFEST_URL` endpoint years ago in favor of a
/// versioned resource-bundle system, and most of the video URLs it still
/// lists now 404 (see `filter_live_assets`). This community-maintained
/// mirror uses the same schema but is kept up to date against Apple's
/// current tvOS resource bundles, so it's merged in on every refresh to
/// cover the entries Apple's own endpoint no longer serves.
pub const COMMUNITY_MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/kopiro/xscreensaver-apple-aerial/main/entries.json";

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

async fn fetch_community_manifest(client: &reqwest::Client) -> Vec<AerialAsset> {
    let body = match client.get(COMMUNITY_MANIFEST_URL).send().await {
        Ok(resp) => match resp.text().await {
            Ok(body) => body,
            Err(err) => {
                tracing::warn!("failed to read community Aerial manifest body: {err}");
                return Vec::new();
            }
        },
        Err(err) => {
            tracing::warn!("failed to fetch community Aerial manifest: {err}");
            return Vec::new();
        }
    };
    match parse_manifest(&body) {
        Ok(data) => data.assets,
        Err(err) => {
            tracing::warn!("failed to parse community Aerial manifest: {err}");
            Vec::new()
        }
    }
}

/// Merges `community` assets into `primary`, keeping `primary`'s entry
/// whenever the same id appears in both (Apple's own metadata is treated as
/// authoritative when available).
fn merge_assets(primary: Vec<AerialAsset>, community: Vec<AerialAsset>) -> Vec<AerialAsset> {
    let mut merged = primary;
    let seen: std::collections::HashSet<String> = merged.iter().map(|a| a.id.clone()).collect();
    for asset in community {
        if !seen.contains(&asset.id) {
            merged.push(asset);
        }
    }
    merged
}

/// Fetches Apple's official manifest (or the bundled fallback on error) and
/// the community mirror, merging the two into one asset list. Individual
/// video liveness is checked separately by `filter_live_assets`.
pub async fn fetch_combined_manifest(client: &reqwest::Client) -> Vec<AerialAsset> {
    let (apple, community) = tokio::join!(
        fetch_manifest_or_fallback(client),
        fetch_community_manifest(client),
    );
    merge_assets(apple.assets, community)
}

/// Selects the download URL for `asset` matching the QML `Quality` enum
/// (0=1080 SDR, 1=1080 HDR, 2=4K SDR, 3=4K HDR), mirroring `urlForQuality` in
/// main.qml and the `Quality` qenum in qml.rs.
pub fn quality_url(asset: &AerialAsset, quality: i32) -> &str {
    match quality {
        1 => &asset.url_1080_hdr,
        2 => &asset.url_4k_sdr,
        3 => &asset.url_4k_hdr,
        _ => &asset.url_1080_sdr,
    }
}

/// Apple's official manifest at `MANIFEST_URL` is fetched successfully as
/// JSON (so `fetch_manifest_or_fallback` never falls back) but has been
/// observed to list mostly-dead video URLs -- Apple deprecated this flat
/// endpoint years ago in favor of a versioned resource-bundle system, and
/// most per-asset video files it still points at 404. Left unfiltered, the
/// wallpaper's playlist ends up almost entirely of entries that fail to
/// download, so playback always falls back to the one or two that still
/// resolve. This does a HEAD check per asset (at the currently selected
/// quality) and drops only the ones confirmed dead, so the rest of the app
/// only ever sees playable entries.
///
/// A HEAD request that errors out (timeout, DNS hiccup, offline) is treated
/// as "unknown" rather than "dead" -- a transient network blip must not wipe
/// out the whole catalog.
pub async fn filter_live_assets(
    client: &reqwest::Client,
    assets: Vec<AerialAsset>,
    quality: i32,
) -> Vec<AerialAsset> {
    let checks = assets.iter().map(|asset| {
        let client = client.clone();
        let url = quality_url(asset, quality).to_string();
        async move {
            match client.head(&url).send().await {
                Ok(resp) => resp.status().is_success(),
                Err(_) => true,
            }
        }
    });
    let alive_flags = futures_util::future::join_all(checks).await;
    let live: Vec<AerialAsset> = assets
        .iter()
        .cloned()
        .zip(alive_flags)
        .filter_map(|(asset, alive)| alive.then_some(asset))
        .collect();
    if live.is_empty() {
        tracing::warn!("all manifest entries failed liveness check; keeping unfiltered list");
        return assets;
    }
    live
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn asset(id: &str, url: &str) -> AerialAsset {
        AerialAsset {
            id: id.to_string(),
            accessibility_label: id.to_string(),
            url_1080_sdr: url.to_string(),
            url_1080_hdr: url.to_string(),
            url_4k_sdr: url.to_string(),
            url_4k_hdr: url.to_string(),
        }
    }

    #[test]
    fn merge_assets_prefers_primary_and_dedupes_by_id() {
        let primary = vec![asset("a", "https://apple/a.mov")];
        let community = vec![
            asset("a", "https://community/a.mov"),
            asset("b", "https://community/b.mov"),
        ];

        let merged = merge_assets(primary, community);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "a");
        assert_eq!(merged[0].url_1080_sdr, "https://apple/a.mov");
        assert_eq!(merged[1].id, "b");
    }

    #[tokio::test]
    async fn filter_live_assets_drops_dead_entries() {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(|req: &wiremock::Request| {
                if req.url.path() == "/live.mov" {
                    ResponseTemplate::new(200)
                } else {
                    ResponseTemplate::new(404)
                }
            })
            .mount(&server)
            .await;

        let assets = vec![
            asset("live", &format!("{}/live.mov", server.uri())),
            asset("dead", &format!("{}/dead.mov", server.uri())),
        ];

        let live = filter_live_assets(&reqwest::Client::new(), assets, 0).await;
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].id, "live");
    }

    #[tokio::test]
    async fn filter_live_assets_keeps_all_when_every_check_fails() {
        // No mock mounted at all: every HEAD request errors out (connection
        // refused), which must be treated as "unknown", not "dead" -- a
        // network blip should never wipe out the whole catalog.
        let assets = vec![
            asset("a", "https://127.0.0.1:1/a.mov"),
            asset("b", "https://127.0.0.1:1/b.mov"),
        ];

        let live = filter_live_assets(&reqwest::Client::new(), assets, 0).await;
        assert_eq!(live.len(), 2);
    }

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
