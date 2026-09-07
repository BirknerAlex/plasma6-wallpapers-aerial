use std::io::Read;

use futures_util::StreamExt;
use serde::Deserialize;
use thiserror::Error;

/// Apple's current (tvOS 26) Aerial catalog, published as a tar archive of the
/// whole resource bundle rather than the flat `entries.json` document it used
/// up to tvOS 11.
///
/// The old `https://sylvan.apple.com/Aerials/2x/entries.json` endpoint is
/// still reachable, but it describes the tvOS 11 catalog whose per-asset
/// `sylvan.apple.com/Aerials/2x/Videos/...` files Apple has almost entirely
/// removed -- in testing only one or two of its entries still resolved, which
/// is what the (retained) `filter_live_assets` liveness check was papering
/// over. tvOS 12 onwards ships `resources*.tar`, whose embedded
/// `entries.json` points at versioned `itunes-assets` URLs that are actually
/// served.
///
/// Both URLs below are the same tvOS 26 bundle, tried in order:
///
/// 1. the fully-qualified `itunes-assets` path an Apple TV itself requests --
///    this is the one documented for tvOS 26, and the one to keep working;
/// 2. the short `/Aerials/`-rooted path, which follows the naming pattern
///    older generations used and has been reported to serve the same file.
///    It is second because it is corroborated only by that report.
///
/// Feed URLs for every generation are catalogued in
/// <https://gist.github.com/theothernt/57a51cade0c12c407f48a5121e0939d5>.
pub const RESOURCES_TAR_URLS: &[&str] = &[
    "https://sylvan.apple.com/itunes-assets/Aerials126/v4/c0/45/d9/c045d9d0-9606-1535-62fe-189edb4f79eb/resources-atv-23J-2.tar",
    "https://sylvan.apple.com/Aerials/resources-atv-23J-2.tar",
];

/// Name of the catalog member inside the resource tar. It has sat at the
/// archive root in every generation so far, but it's matched by file name
/// anywhere in the archive so a future re-layout doesn't break parsing.
const ENTRIES_MEMBER_NAME: &str = "entries.json";

/// Hard ceiling on how much of a resource tar we buffer. The real archive is
/// well under a megabyte (metadata only -- the videos themselves are
/// downloaded separately, on demand); this only exists so a redirect to
/// something enormous can't balloon plasmashell's memory.
const MAX_TAR_BYTES: usize = 16 * 1024 * 1024;

/// Same idea for the extracted `entries.json` member, whose declared size in
/// the tar header is attacker/CDN-controlled.
const MAX_ENTRIES_JSON_BYTES: u64 = 16 * 1024 * 1024;

/// How many liveness HEAD requests to keep in flight at once. The tvOS 26
/// catalog has ~140 entries and firing all of them at Apple simultaneously
/// invites throttling (which `filter_live_assets` would read as "unknown",
/// not "dead", but it still wastes time and sockets).
const LIVENESS_CONCURRENCY: usize = 16;

/// Snapshot of Apple's tvOS 26 catalog, trimmed to the fields this crate
/// uses, embedded so the wallpaper still has a playlist when Apple (or the
/// network) is unreachable at startup. See CONTRIBUTING.md for how to
/// regenerate it.
const FALLBACK_MANIFEST: &str = include_str!("../assets/entries.fallback.json");

/// One playable Aerial clip, normalized from whatever Apple's catalog
/// happened to say (see [`RawAerialAsset`]): a non-empty label and, for each
/// quality tier, either a URL or an empty string.
#[derive(Debug, Clone, PartialEq)]
pub struct AerialAsset {
    pub id: String,
    pub accessibility_label: String,
    /// Apple's `timeOfDay` for this clip, lowercased: `day`, `night`,
    /// `sunrise` or `sunset` in the tvOS 26 catalog, empty when the entry
    /// doesn't say (older bundles, or a value we've never seen). Drives the
    /// time-of-day playlist filter -- see [`time_of_day_matches`].
    pub time_of_day: String,
    /// Apple's still preview for this clip (a 900x580 PNG on
    /// `sylvan.apple.com`), empty when the entry doesn't offer one. Shown as
    /// a thumbnail in the wallpaper's config dialog.
    pub preview_image: String,
    pub url_1080_sdr: String,
    pub url_1080_hdr: String,
    pub url_4k_sdr: String,
    pub url_4k_hdr: String,
}

/// Wire representation of one entry as Apple actually publishes it.
///
/// Every field except `id` is optional: the tvOS 26 catalog carries a pile of
/// extra keys this crate ignores (`categories`, `pointsOfInterest`, `scene`,
/// `timeOfDay`, `previewImage`, ...), and not every entry offers every
/// quality tier -- newer bundles have started shipping variants such as
/// `url-4K-SDR-240FPS` in place of some of the classic four. Deserializing
/// permissively and normalizing afterwards means one missing key can't throw
/// away the entire catalog.
#[derive(Debug, Deserialize)]
struct RawAerialAsset {
    id: String,
    #[serde(rename = "accessibilityLabel")]
    accessibility_label: Option<String>,
    #[serde(rename = "localizedNameKey")]
    localized_name_key: Option<String>,
    #[serde(rename = "shotID")]
    shot_id: Option<String>,
    #[serde(rename = "timeOfDay")]
    time_of_day: Option<String>,
    #[serde(rename = "previewImage")]
    preview_image: Option<String>,
    // Some bundles carry the preview under an explicitly-sized key instead;
    // in the tvOS 26 catalog it is present but always empty.
    #[serde(rename = "previewImage-900x580")]
    preview_image_900x580: Option<String>,
    #[serde(rename = "url-1080-SDR")]
    url_1080_sdr: Option<String>,
    #[serde(rename = "url-1080-HDR")]
    url_1080_hdr: Option<String>,
    #[serde(rename = "url-4K-SDR")]
    url_4k_sdr: Option<String>,
    #[serde(rename = "url-4K-SDR-240FPS")]
    url_4k_sdr_240fps: Option<String>,
    #[serde(rename = "url-4K-HDR")]
    url_4k_hdr: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawAerialManifest {
    #[serde(default)]
    version: u32,
    #[serde(rename = "initialAssetCount", default)]
    initial_asset_count: u32,
    #[serde(default)]
    assets: Vec<RawAerialAsset>,
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

impl RawAerialAsset {
    /// Drops entries with no playable URL at all and picks a display label,
    /// preferring Apple's own human-readable one.
    fn normalize(self) -> Option<AerialAsset> {
        let label = non_empty(self.accessibility_label)
            .or_else(|| non_empty(self.shot_id))
            .or_else(|| non_empty(self.localized_name_key))
            .unwrap_or_else(|| self.id.clone());

        let asset = AerialAsset {
            id: self.id,
            accessibility_label: label,
            time_of_day: non_empty(self.time_of_day)
                .map(|value| value.trim().to_ascii_lowercase())
                .unwrap_or_default(),
            preview_image: non_empty(self.preview_image)
                .or_else(|| non_empty(self.preview_image_900x580))
                .map(|value| value.trim().to_string())
                .unwrap_or_default(),
            url_1080_sdr: non_empty(self.url_1080_sdr).unwrap_or_default(),
            url_1080_hdr: non_empty(self.url_1080_hdr).unwrap_or_default(),
            url_4k_sdr: non_empty(self.url_4k_sdr)
                .or_else(|| non_empty(self.url_4k_sdr_240fps))
                .unwrap_or_default(),
            url_4k_hdr: non_empty(self.url_4k_hdr).unwrap_or_default(),
        };
        if asset.id.trim().is_empty() || !asset.has_any_url() {
            return None;
        }
        Some(asset)
    }
}

impl AerialAsset {
    fn has_any_url(&self) -> bool {
        !self.url_1080_sdr.is_empty()
            || !self.url_1080_hdr.is_empty()
            || !self.url_4k_sdr.is_empty()
            || !self.url_4k_hdr.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AerialManifestData {
    pub version: u32,
    pub initial_asset_count: u32,
    pub assets: Vec<AerialAsset>,
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("network request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("failed to parse manifest json: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("failed to read resource tar: {0}")]
    Tar(#[from] std::io::Error),
    #[error("resource tar contains no {ENTRIES_MEMBER_NAME}")]
    MissingEntries,
    #[error("resource download exceeded {MAX_TAR_BYTES} bytes")]
    TooLarge,
    #[error("no manifest source could be reached")]
    NoSource,
}

/// Parses a bare `entries.json` document (the shape Apple ships inside the
/// resource tar, and the shape of the bundled fallback snapshot).
pub fn parse_manifest(body: &str) -> Result<AerialManifestData, ManifestError> {
    let raw: RawAerialManifest = serde_json::from_str(body)?;
    let assets: Vec<AerialAsset> = raw
        .assets
        .into_iter()
        .filter_map(RawAerialAsset::normalize)
        .collect();
    Ok(AerialManifestData {
        version: raw.version,
        initial_asset_count: raw.initial_asset_count,
        assets,
    })
}

/// Pulls the `entries.json` member out of a `resources*.tar` bundle. The
/// archive also carries the localized-strings bundle and other resources,
/// which are ignored.
pub fn extract_entries_json(tar_bytes: &[u8]) -> Result<String, ManifestError> {
    let mut archive = tar::Archive::new(std::io::Cursor::new(tar_bytes));
    for entry in archive.entries()? {
        let entry = entry?;
        let is_entries = entry
            .path()
            .ok()
            .and_then(|path| path.file_name().map(|name| name == ENTRIES_MEMBER_NAME))
            .unwrap_or(false);
        if !is_entries {
            continue;
        }
        let mut body = String::new();
        entry
            .take(MAX_ENTRIES_JSON_BYTES)
            .read_to_string(&mut body)?;
        return Ok(body);
    }
    Err(ManifestError::MissingEntries)
}

/// Parses a whole `resources*.tar` bundle into the asset catalog.
pub fn parse_resources_tar(tar_bytes: &[u8]) -> Result<AerialManifestData, ManifestError> {
    parse_manifest(&extract_entries_json(tar_bytes)?)
}

pub fn fallback_manifest() -> AerialManifestData {
    parse_manifest(FALLBACK_MANIFEST).expect("bundled fallback manifest must be valid JSON")
}

/// Downloads a resource tar, refusing anything implausibly large rather than
/// buffering it all into plasmashell's address space.
async fn fetch_tar(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, ManifestError> {
    let response = client.get(url).send().await?.error_for_status()?;
    let mut body: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len() + chunk.len() > MAX_TAR_BYTES {
            return Err(ManifestError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn fetch_manifest_from(
    client: &reqwest::Client,
    urls: &[&str],
) -> Result<AerialManifestData, ManifestError> {
    let mut last_error = ManifestError::NoSource;
    for url in urls {
        match fetch_tar(client, url).await {
            Ok(bytes) => match parse_resources_tar(&bytes) {
                Ok(data) => return Ok(data),
                Err(err) => {
                    tracing::warn!("failed to parse Aerial resource bundle from {url}: {err}");
                    last_error = err;
                }
            },
            Err(err) => {
                tracing::warn!("failed to fetch Aerial resource bundle from {url}: {err}");
                last_error = err;
            }
        }
    }
    Err(last_error)
}

/// Fetches Apple's current resource bundle, trying each known URL in turn.
pub async fn fetch_manifest(client: &reqwest::Client) -> Result<AerialManifestData, ManifestError> {
    fetch_manifest_from(client, RESOURCES_TAR_URLS).await
}

/// Fetches the live manifest, falling back to the bundled snapshot on any error.
pub async fn fetch_manifest_or_fallback(client: &reqwest::Client) -> AerialManifestData {
    match fetch_manifest(client).await {
        Ok(data) if !data.assets.is_empty() => data,
        Ok(_) => {
            tracing::warn!(
                "Aerial resource bundle listed no usable assets; using bundled snapshot"
            );
            fallback_manifest()
        }
        Err(err) => {
            tracing::warn!("falling back to bundled Aerial manifest: {err}");
            fallback_manifest()
        }
    }
}

/// Asset list handed to the QML layer. Individual video liveness is checked
/// separately by `filter_live_assets`.
pub async fn fetch_assets(client: &reqwest::Client) -> Vec<AerialAsset> {
    fetch_manifest_or_fallback(client).await.assets
}

/// Selects the download URL for `asset` matching the QML `Quality` enum
/// (0=1080 SDR, 1=1080 HDR, 2=4K SDR, 3=4K HDR), mirroring `urlForQuality` in
/// main.qml and the `Quality` qenum in qml.rs.
///
/// Apple does not guarantee every tier for every clip, so a missing one falls
/// back to whatever that entry does offer -- playing a clip at the wrong
/// resolution beats a playlist entry that can never load.
pub fn quality_url(asset: &AerialAsset, quality: i32) -> &str {
    let preferred = match quality {
        1 => &asset.url_1080_hdr,
        2 => &asset.url_4k_sdr,
        3 => &asset.url_4k_hdr,
        _ => &asset.url_1080_sdr,
    };
    if !preferred.is_empty() {
        return preferred;
    }
    [
        &asset.url_1080_sdr,
        &asset.url_1080_hdr,
        &asset.url_4k_sdr,
        &asset.url_4k_hdr,
    ]
    .into_iter()
    .find(|url| !url.is_empty())
    .map(String::as_str)
    .unwrap_or("")
}

/// Time-of-day playlist filter, matching the `TimeOfDay` enum in
/// `package/contents/config/main.xml`.
pub mod time_filter {
    /// Every clip, whatever Apple tagged it (the default).
    pub const ALL: i32 = 0;
    /// Daylight clips: Apple's `day`, plus `sunrise`.
    pub const DAY: i32 = 1;
    /// After-dark clips: Apple's `night`, plus `sunset`.
    pub const NIGHT: i32 = 2;
    /// Whatever matches the viewer's own clock right now.
    pub const AUTO: i32 = 3;
}

/// Maps the local hour (0-23) to the `timeOfDay` bucket Apple would tag a
/// clip shot at that hour with.
///
/// This is a fixed-hours approximation, not real solar times: the wallpaper
/// has no location permission (and no business asking for one), so actual
/// sunrise/sunset are not available. The windows are deliberately generous --
/// being an hour off picks a clip of the adjacent mood, which is the whole
/// point of the setting anyway.
fn bucket_for_hour(hour: i32) -> &'static str {
    match hour.rem_euclid(24) {
        5..=7 => "sunrise",
        8..=17 => "day",
        18..=20 => "sunset",
        _ => "night",
    }
}

/// Whether `time_of_day` (an [`AerialAsset::time_of_day`] value) belongs in a
/// playlist filtered with `mode` at local clock hour `hour`.
///
/// Entries Apple didn't tag always match: a filter that silently hid every
/// untagged clip would empty the playlist against an older resource bundle.
/// In [`time_filter::AUTO`] the exact bucket for the hour is preferred, but
/// its day/night half is accepted too -- Apple ships only 7 sunrise and 18
/// sunset clips, so an exact-only match would loop over a handful of videos
/// (and repeat them constantly) for several hours a day.
pub fn time_of_day_matches(time_of_day: &str, mode: i32, hour: i32) -> bool {
    let tag = time_of_day.trim().to_ascii_lowercase();
    if tag.is_empty() {
        return true;
    }
    let is_day = matches!(tag.as_str(), "day" | "sunrise");
    let is_night = matches!(tag.as_str(), "night" | "sunset");
    match mode {
        time_filter::DAY => is_day,
        time_filter::NIGHT => is_night,
        time_filter::AUTO => {
            let bucket = bucket_for_hour(hour);
            let bucket_is_day = matches!(bucket, "day" | "sunrise");
            tag == bucket || (bucket_is_day && is_day) || (!bucket_is_day && is_night)
        }
        // Unknown mode ordinals (a config written by a newer version) behave
        // like ALL rather than blanking the wallpaper.
        _ => true,
    }
}

/// Applies [`time_of_day_matches`] to a whole catalog, keeping the unfiltered
/// list if the filter would leave nothing to play.
pub fn filter_by_time_of_day(assets: Vec<AerialAsset>, mode: i32, hour: i32) -> Vec<AerialAsset> {
    if mode == time_filter::ALL {
        return assets;
    }
    let filtered: Vec<AerialAsset> = assets
        .iter()
        .filter(|asset| time_of_day_matches(&asset.time_of_day, mode, hour))
        .cloned()
        .collect();
    if filtered.is_empty() {
        tracing::warn!("time-of-day filter {mode} matched no entries; keeping unfiltered list");
        return assets;
    }
    filtered
}

/// Drops entries whose video file at the selected quality is confirmed gone.
///
/// Apple's catalog and its CDN drift apart over time (this was severe on the
/// old tvOS 11 `entries.json`, where nearly every listed video 404'd; the
/// tvOS 26 bundle is far healthier but still not guaranteed), and a playlist
/// full of entries that fail to download means playback keeps stalling and
/// skipping. This does a HEAD check per asset and removes only the ones the
/// server actively reports as missing.
///
/// A HEAD request that errors out (timeout, DNS hiccup, offline) is treated
/// as "unknown" rather than "dead" -- a transient network blip must not wipe
/// out the whole catalog.
pub async fn filter_live_assets(
    client: &reqwest::Client,
    assets: Vec<AerialAsset>,
    quality: i32,
) -> Vec<AerialAsset> {
    // URLs are collected up front so each check owns its data -- the futures
    // must not borrow `assets`, which is still needed (and moved) below.
    let urls: Vec<String> = assets
        .iter()
        .map(|asset| quality_url(asset, quality).to_string())
        .collect();
    let checks = urls.into_iter().map(|url| {
        let client = client.clone();
        async move {
            match client.head(&url).send().await {
                Ok(resp) => resp.status().is_success(),
                Err(_) => true,
            }
        }
    });
    // `buffered` (not `buffer_unordered`) so results stay aligned with
    // `assets` below, while capping how many requests hit Apple at once.
    let alive_flags: Vec<bool> = futures_util::stream::iter(checks)
        .buffered(LIVENESS_CONCURRENCY)
        .collect()
        .await;
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn asset(id: &str, url: &str) -> AerialAsset {
        AerialAsset {
            id: id.to_string(),
            accessibility_label: id.to_string(),
            time_of_day: String::new(),
            preview_image: String::new(),
            url_1080_sdr: url.to_string(),
            url_1080_hdr: url.to_string(),
            url_4k_sdr: url.to_string(),
            url_4k_hdr: url.to_string(),
        }
    }

    /// Builds an in-memory tar shaped like Apple's resource bundle: the
    /// localized-strings bundle first, `entries.json` somewhere after it.
    fn resources_tar(members: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, contents) in members {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, name, contents.as_bytes())
                .expect("in-memory tar append must succeed");
        }
        builder.into_inner().expect("tar finish must succeed")
    }

    const TVOS26_FIXTURE: &str = include_str!("../tests/fixtures/entries-tvos26.json");
    const LEGACY_FIXTURE: &str = include_str!("../tests/fixtures/entries.json");

    #[test]
    fn parses_tvos26_entries_fixture() {
        let data = parse_manifest(TVOS26_FIXTURE).expect("fixture must parse");
        assert_eq!(data.assets.len(), 2);

        let korea = &data.assets[0];
        assert_eq!(korea.id, "009BA758-7060-4479-8EE8-FB9B40C8FB97");
        assert_eq!(korea.accessibility_label, "Korea and Japan Night");
        assert!(korea
            .url_1080_sdr
            .starts_with("https://sylvan.apple.com/itunes-assets/"));
        assert!(korea.url_4k_hdr.ends_with("_HDR_4K_HEVC.mov"));
        assert_eq!(data.assets[1].accessibility_label, "Antarctica");

        assert_eq!(korea.time_of_day, "night");
        assert_eq!(data.assets[1].time_of_day, "sunrise");

        assert!(korea.preview_image.starts_with("https://sylvan.apple.com/"));
        assert!(korea.preview_image.ends_with(".png"));
    }

    #[test]
    fn preview_image_falls_back_to_the_sized_key() {
        let body = r#"{
            "assets": [
                {
                    "id": "sized-only",
                    "url-1080-SDR": "https://apple/a.mov",
                    "previewImage": "",
                    "previewImage-900x580": "https://apple/a.png"
                }
            ]
        }"#;

        let data = parse_manifest(body).expect("permissive schema must parse");
        assert_eq!(data.assets[0].preview_image, "https://apple/a.png");
    }

    #[test]
    fn parses_legacy_flat_schema_fixture() {
        let data = parse_manifest(LEGACY_FIXTURE).expect("fixture must parse");
        assert_eq!(data.version, 1);
        assert_eq!(data.initial_asset_count, 2);
        assert_eq!(data.assets.len(), 2);
        assert_eq!(data.assets[0].accessibility_label, "Los Angeles");
    }

    #[test]
    fn parse_tolerates_missing_optional_fields() {
        let body = r#"{
            "assets": [
                { "id": "no-label", "url-4K-SDR": "https://apple/a.mov" },
                { "id": "no-urls", "accessibilityLabel": "Nowhere" },
                { "id": "shot-id-label", "shotID": "GMT026", "url-1080-SDR": "https://apple/b.mov" }
            ]
        }"#;

        let data = parse_manifest(body).expect("permissive schema must parse");
        assert_eq!(data.assets.len(), 2, "entries without any URL are dropped");
        assert_eq!(data.assets[0].accessibility_label, "no-label");
        assert_eq!(data.assets[0].url_1080_sdr, "");
        assert_eq!(data.assets[1].accessibility_label, "GMT026");
    }

    #[test]
    fn four_k_sdr_prefers_classic_url_and_falls_back_to_240fps() {
        let body = r#"{
            "assets": [
                {
                    "id": "both",
                    "url-4K-SDR": "https://apple/classic.mov",
                    "url-4K-SDR-240FPS": "https://apple/240fps.mov"
                },
                {
                    "id": "240fps-only",
                    "url-4K-SDR-240FPS": "https://apple/only-240fps.mov"
                }
            ]
        }"#;

        let data = parse_manifest(body).expect("240FPS SDR variants must parse");
        assert_eq!(data.assets.len(), 2, "240FPS-only entries must be retained");
        assert_eq!(data.assets[0].url_4k_sdr, "https://apple/classic.mov");
        assert_eq!(data.assets[1].url_4k_sdr, "https://apple/only-240fps.mov");
    }

    #[test]
    fn extracts_entries_json_from_resource_tar() {
        let tar = resources_tar(&[
            (
                "TVIdleScreenStringsBundle.bundle/en.lproj/Localizable.nocache.strings",
                "\"KEY\" = \"Value\";",
            ),
            ("entries.json", TVOS26_FIXTURE),
        ]);

        let data = parse_resources_tar(&tar).expect("tar must parse");
        assert_eq!(data.assets.len(), 2);
        assert_eq!(data.assets[0].accessibility_label, "Korea and Japan Night");
    }

    #[test]
    fn resource_tar_without_entries_json_is_an_error() {
        let tar = resources_tar(&[("README.txt", "nothing to see here")]);
        assert!(matches!(
            parse_resources_tar(&tar),
            Err(ManifestError::MissingEntries)
        ));
    }

    #[tokio::test]
    async fn fetch_manifest_falls_through_to_the_next_url() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/gone.tar"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/mirror.tar"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(resources_tar(&[("entries.json", TVOS26_FIXTURE)])),
            )
            .mount(&server)
            .await;

        let urls = [
            format!("{}/gone.tar", server.uri()),
            format!("{}/mirror.tar", server.uri()),
        ];
        let urls: Vec<&str> = urls.iter().map(String::as_str).collect();

        let data = fetch_manifest_from(&reqwest::Client::new(), &urls)
            .await
            .expect("second URL must be used after the first 404s");
        assert_eq!(data.assets.len(), 2);
    }

    #[tokio::test]
    async fn fetch_manifest_or_fallback_uses_bundled_snapshot_when_offline() {
        let data =
            fetch_manifest_from(&reqwest::Client::new(), &["https://127.0.0.1:1/x.tar"]).await;
        assert!(data.is_err());

        let fallback = fallback_manifest();
        assert!(
            fallback.assets.len() > 100,
            "snapshot must cover the catalog"
        );
        assert!(fallback
            .assets
            .iter()
            .all(|a| a.url_4k_hdr.starts_with("https://")));
    }

    #[test]
    fn quality_url_falls_back_to_an_available_tier() {
        let mut only_4k_sdr = asset("a", "");
        only_4k_sdr.url_4k_sdr = "https://apple/4k-sdr.mov".to_string();

        // 1080 SDR requested but absent -> use whatever the entry does have.
        assert_eq!(quality_url(&only_4k_sdr, 0), "https://apple/4k-sdr.mov");
        assert_eq!(quality_url(&only_4k_sdr, 2), "https://apple/4k-sdr.mov");
        assert_eq!(quality_url(&asset("b", ""), 0), "");
    }

    fn dated_asset(id: &str, time_of_day: &str) -> AerialAsset {
        let mut asset = asset(id, "https://apple/clip.mov");
        asset.time_of_day = time_of_day.to_string();
        asset
    }

    #[test]
    fn time_of_day_filter_groups_twilight_with_its_half_of_the_day() {
        // Hour is irrelevant unless the mode is AUTO.
        for tag in ["day", "sunrise"] {
            assert!(time_of_day_matches(tag, time_filter::DAY, 0));
            assert!(!time_of_day_matches(tag, time_filter::NIGHT, 0));
        }
        for tag in ["night", "sunset"] {
            assert!(time_of_day_matches(tag, time_filter::NIGHT, 0));
            assert!(!time_of_day_matches(tag, time_filter::DAY, 0));
        }
    }

    #[test]
    fn time_of_day_filter_keeps_everything_it_cannot_classify() {
        // ALL, an unknown mode, an untagged entry: never hide anything.
        assert!(time_of_day_matches("night", time_filter::ALL, 12));
        assert!(time_of_day_matches("night", 99, 12));
        assert!(time_of_day_matches("", time_filter::DAY, 12));
        assert!(time_of_day_matches("", time_filter::NIGHT, 12));
    }

    #[test]
    fn auto_mode_follows_the_local_clock() {
        // Midday: daylight clips, including sunrise, not after-dark ones.
        assert!(time_of_day_matches("day", time_filter::AUTO, 13));
        assert!(time_of_day_matches("sunrise", time_filter::AUTO, 13));
        assert!(!time_of_day_matches("night", time_filter::AUTO, 13));

        // Small hours: the reverse.
        assert!(time_of_day_matches("night", time_filter::AUTO, 2));
        assert!(time_of_day_matches("sunset", time_filter::AUTO, 2));
        assert!(!time_of_day_matches("day", time_filter::AUTO, 2));

        // Dawn and dusk sit in the half of the day they lead into.
        assert!(time_of_day_matches("sunrise", time_filter::AUTO, 6));
        assert!(time_of_day_matches("day", time_filter::AUTO, 6));
        assert!(!time_of_day_matches("sunset", time_filter::AUTO, 6));
        assert!(time_of_day_matches("sunset", time_filter::AUTO, 19));
        assert!(time_of_day_matches("night", time_filter::AUTO, 19));
        assert!(!time_of_day_matches("sunrise", time_filter::AUTO, 19));

        // A QML `Date.getHours()` can only be 0-23, but don't blank the
        // wallpaper if something ever passes nonsense.
        assert!(time_of_day_matches("day", time_filter::AUTO, 36));
        assert!(time_of_day_matches("night", time_filter::AUTO, -2));
    }

    #[test]
    fn filter_by_time_of_day_selects_the_matching_half() {
        let assets = vec![
            dated_asset("d", "day"),
            dated_asset("sr", "sunrise"),
            dated_asset("n", "night"),
            dated_asset("ss", "sunset"),
        ];

        let day = filter_by_time_of_day(assets.clone(), time_filter::DAY, 12);
        assert_eq!(
            day.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            ["d", "sr"]
        );

        let all = filter_by_time_of_day(assets.clone(), time_filter::ALL, 12);
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn filter_by_time_of_day_keeps_the_catalog_rather_than_emptying_it() {
        // A catalog with no night footage must not leave the wallpaper with
        // an empty playlist (and a black screen) all night.
        let assets = vec![dated_asset("d", "day"), dated_asset("d2", "day")];
        let night = filter_by_time_of_day(assets, time_filter::NIGHT, 23);
        assert_eq!(night.len(), 2);
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

    #[test]
    fn bundled_fallback_manifest_is_valid() {
        let data = fallback_manifest();
        assert!(!data.assets.is_empty());
        // The snapshot is regenerated by hand (see CONTRIBUTING.md); dropping
        // `timeOfDay` from it would silently disable the time-of-day filter
        // whenever the wallpaper starts offline.
        assert!(data
            .assets
            .iter()
            .all(|asset| !asset.time_of_day.is_empty()));
        assert!(data.assets.iter().any(|asset| asset.time_of_day == "night"));
        // Same for the preview images the config dialog shows.
        assert!(data
            .assets
            .iter()
            .all(|asset| asset.preview_image.starts_with("https://")));
    }
}
