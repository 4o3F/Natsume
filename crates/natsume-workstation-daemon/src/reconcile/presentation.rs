//! Display-only persistence and HTTP image work. No resource target is read from this cache.

use std::{
    fs,
    io::Cursor,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use image::{ImageFormat, ImageReader};
use natsume_workstation_ipc::WaitingTeam;
use natsume_workstation_protocol::generated::{BoundTarget, ServerStateSnapshot};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::watch;

use crate::{
    atomic_write::{WritePolicy, atomic_write},
    canonical_uuid_v7,
};

use super::{SnapshotError, binding::BindingInputProvider};

const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DelayReason {
    Initial,
    Periodic,
    Change,
}

fn delay_from_sample(reason: DelayReason, sample: u32) -> Option<Duration> {
    let (minimum, buckets) = match reason {
        DelayReason::Initial => (0, 60_001),
        DelayReason::Periodic => (45_000, 30_001),
        DelayReason::Change => (0, 5_001),
    };
    let sample_space = u64::from(u32::MAX) + 1;
    let limit = sample_space - sample_space % buckets;
    let sample = u64::from(sample);
    (sample < limit).then(|| Duration::from_millis(minimum + sample % buckets))
}

fn random_delay(reason: DelayReason) -> Duration {
    loop {
        let Ok(sample) = getrandom::u32() else {
            tracing::warn!(
                code = "logo_poll_entropy_unavailable",
                "Waiting logo polling entropy unavailable; using fallback delay"
            );
            return match reason {
                DelayReason::Periodic => Duration::from_secs(60),
                DelayReason::Initial | DelayReason::Change => Duration::from_secs(5),
            };
        };
        if let Some(delay) = delay_from_sample(reason, sample) {
            return delay;
        }
    }
}

pub(super) fn validate_bound(bound: &BoundTarget) -> Option<WaitingTeam> {
    let context = bound.context.as_ref()?;
    let presentation = bound.presentation.as_ref()?;
    let team = WaitingTeam {
        binding_id: context.binding_id.clone(),
        account_id: context.account_id.clone(),
        seat_code: context.seat_code.clone(),
        organization_id: presentation.organization_id.clone(),
        team_name_zh: presentation.team_name_zh.clone(),
        team_name_en: presentation.team_name_en.clone(),
        school_name_zh: presentation.school_name_zh.clone(),
        school_name_en: presentation.school_name_en.clone(),
    };
    valid_team(&team).then_some(team)
}

fn valid_team(team: &WaitingTeam) -> bool {
    let organization = team
        .organization_id
        .strip_prefix("INST-")
        .and_then(|id| id.parse::<i64>().ok());
    canonical_uuid_v7(&team.binding_id).is_some()
        && canonical_uuid_v7(&team.account_id).is_some()
        && !team.seat_code.is_empty()
        && team.seat_code.len() <= 64
        && !team.seat_code.chars().any(char::is_control)
        && organization.is_some_and(|id| id > 0 && team.organization_id == format!("INST-{id:03}"))
        && (!team.team_name_zh.is_empty() || !team.team_name_en.is_empty())
        && (!team.school_name_zh.is_empty() || !team.school_name_en.is_empty())
        && [
            &team.team_name_zh,
            &team.team_name_en,
            &team.school_name_zh,
            &team.school_name_en,
        ]
        .iter()
        .all(|name| {
            name.len() <= 1024
                && name.trim() == name.as_str()
                && !name.chars().any(char::is_control)
        })
}

/// Called only after semantic validation of the *entire* server snapshot.
pub(super) fn from_snapshot(snapshot: &ServerStateSnapshot) -> Option<WaitingTeam> {
    snapshot
        .target
        .as_ref()?
        .binding_access
        .as_ref()?
        .bound
        .as_ref()
        .and_then(validate_bound)
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Cache {
    format_version: u32,
    scope: String,
    team: Option<WaitingTeam>,
    logo_digest: Option<String>,
    etag: Option<String>,
}

struct State {
    cache: Cache,
    offline: bool,
    generation: u64,
}

pub(super) struct Presentation {
    record: PathBuf,
    images: PathBuf,
    provider: Arc<BindingInputProvider>,
    state: Mutex<State>,
    changed: watch::Sender<u64>,
}

#[derive(Clone)]
struct Download {
    generation: u64,
    scope: String,
    team: WaitingTeam,
    etag: Option<String>,
}

impl Presentation {
    pub(super) fn new(
        record: PathBuf,
        images: PathBuf,
        provider: Arc<BindingInputProvider>,
    ) -> Self {
        Self {
            record,
            images,
            provider,
            state: Mutex::new(State {
                cache: Cache {
                    format_version: FORMAT_VERSION,
                    scope: String::new(),
                    team: None,
                    logo_digest: None,
                    etag: None,
                },
                offline: true,
                generation: 0,
            }),
            changed: watch::channel(0).0,
        }
    }

    /// Endpoint, pinned trust root and enrolled device ID are hashed by the control owner.
    pub(super) fn configure(&self, scope: String) -> Result<(), SnapshotError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SnapshotError::PresentationCache)?;
        if state.cache.scope == scope {
            return Ok(());
        }
        let cached = fs::metadata(&self.record)
            .ok()
            .filter(|meta| meta.len() <= 16_384)
            .and_then(|_| fs::read(&self.record).ok())
            .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
            .filter(|cache| {
                cache.format_version == FORMAT_VERSION
                    && cache.scope == scope
                    && cache.team.as_ref().is_none_or(valid_team)
            });
        state.cache = cached.unwrap_or(Cache {
            format_version: FORMAT_VERSION,
            scope,
            team: None,
            logo_digest: None,
            etag: None,
        });
        // A corrupt/missing image is not a corrupt team profile. Never trust a cached path.
        if !self.valid_image(&state.cache) {
            state.cache.logo_digest = None;
            state.cache.etag = None;
        }
        state.offline = true;
        self.advance(&mut state)?;
        self.clean_images(&state.cache);
        Ok(())
    }

    pub(super) fn accept(&self, team: Option<WaitingTeam>) -> Result<(), SnapshotError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SnapshotError::PresentationCache)?;
        let changed = state.cache.team != team;
        if changed || state.offline {
            let same_binding_school =
                state
                    .cache
                    .team
                    .as_ref()
                    .zip(team.as_ref())
                    .is_some_and(|(old, new)| {
                        old.binding_id == new.binding_id
                            && old.account_id == new.account_id
                            && old.organization_id == new.organization_id
                    });
            let mut next = state.cache.clone();
            next.team = team;
            if !same_binding_school {
                next.logo_digest = None;
                next.etag = None;
            }
            // In particular, persist an explicit unbound record before publishing it.
            self.persist(&next)?;
            state.cache = next;
        }
        if changed || state.offline {
            state.offline = false;
            self.advance(&mut state)?;
            self.clean_images(&state.cache);
        }
        Ok(())
    }

    pub(super) fn disconnect(&self) -> Result<(), SnapshotError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SnapshotError::PresentationCache)?;
        if !state.offline {
            state.offline = true;
            self.advance(&mut state)?;
        }
        Ok(())
    }

    fn advance(&self, state: &mut State) -> Result<(), SnapshotError> {
        state.generation = state.generation.saturating_add(1);
        self.provider.set_display(
            state.cache.team.clone(),
            self.image_path(&state.cache)
                .map(|p| p.to_string_lossy().into_owned()),
            state.offline,
            false,
        )?;
        self.changed.send_replace(state.generation);
        Ok(())
    }

    fn image_path(&self, cache: &Cache) -> Option<PathBuf> {
        cache.team.as_ref()?;
        let digest = cache.logo_digest.as_ref()?;
        (digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then(|| self.images.join(format!("{digest}.png")))
    }

    fn image_bytes(&self, cache: &Cache) -> Option<Vec<u8>> {
        let path = self.image_path(cache)?;
        fs::metadata(&path)
            .ok()
            .filter(|meta| meta.len() <= MAX_IMAGE_BYTES as u64)?;
        let bytes = fs::read(path).ok()?;
        (Some(hex::encode(Sha256::digest(&bytes))) == cache.logo_digest).then_some(bytes)
    }

    fn valid_image(&self, cache: &Cache) -> bool {
        self.image_bytes(cache)
            .is_some_and(|bytes| normalize_image(&bytes).is_ok())
    }

    fn persist(&self, cache: &Cache) -> Result<(), SnapshotError> {
        let bytes = serde_json::to_vec(cache).map_err(|_| SnapshotError::PresentationCache)?;
        atomic_write(&self.record, &bytes, 0o600, WritePolicy::Replace)
            .map_err(|_| SnapshotError::PresentationCache)
    }

    fn clean_images(&self, cache: &Cache) {
        let selected = self.image_path(cache);
        if let Ok(entries) = fs::read_dir(&self.images) {
            for entry in entries.flatten() {
                let path = entry.path();
                if Some(&path) != selected.as_ref()
                    && path.extension().is_some_and(|ext| ext == "png")
                    && let Err(error) = fs::remove_file(&path)
                {
                    tracing::warn!(%error, "Obsolete waiting logo cache could not be removed");
                }
            }
        }
    }

    fn download(&self) -> Option<Download> {
        let state = self.state.lock().ok()?;
        (!state.offline).then_some(())?;
        Some(Download {
            generation: state.generation,
            scope: state.cache.scope.clone(),
            team: state.cache.team.clone()?,
            etag: self.image_bytes(&state.cache).and(state.cache.etag.clone()),
        })
    }

    fn finish(
        &self,
        download: &Download,
        image: Option<(Vec<u8>, Option<String>)>,
    ) -> Result<(), SnapshotError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SnapshotError::PresentationCache)?;
        // Cancellation is an optimization; this check fences even a late completed decoder.
        if state.offline
            || state.generation != download.generation
            || state.cache.scope != download.scope
            || state.cache.team.as_ref() != Some(&download.team)
        {
            return Ok(());
        }
        let mut next = state.cache.clone();
        let mut repaired = false;
        if let Some((bytes, etag)) = image {
            let digest = hex::encode(Sha256::digest(&bytes));
            repaired =
                next.logo_digest.as_ref() == Some(&digest) && self.image_bytes(&next).is_none();
            if next.logo_digest.as_ref() != Some(&digest) || repaired {
                let path = self.images.join(format!("{digest}.png"));
                atomic_write(&path, &bytes, 0o644, WritePolicy::Replace)
                    .map_err(|_| SnapshotError::PresentationCache)?;
            }
            next.logo_digest = Some(digest);
            next.etag = etag;
        } else {
            next.logo_digest = None;
            next.etag = None;
        }
        if !repaired && next.logo_digest == state.cache.logo_digest && next.etag == state.cache.etag
        {
            return Ok(());
        }
        self.persist(&next)?;
        state.cache = next;
        self.provider.set_display(
            state.cache.team.clone(),
            self.image_path(&state.cache)
                .map(|p| p.to_string_lossy().into_owned()),
            state.offline,
            repaired,
        )?;
        self.clean_images(&state.cache);
        Ok(())
    }

    /// One in-flight download; target changes interrupt HTTP work. Periodic `ETag` checks
    /// also discover a replaced logo when the control target has not changed.
    pub(super) async fn run(self: Arc<Self>, client: reqwest::Client, origin: String) {
        self.run_with_delays(client, origin, random_delay).await;
    }

    async fn run_with_delays(
        self: Arc<Self>,
        client: reqwest::Client,
        origin: String,
        mut next_delay: impl FnMut(DelayReason) -> Duration,
    ) {
        let mut changes = self.changed.subscribe();
        let mut reason = DelayReason::Initial;
        loop {
            let delay = next_delay(reason);
            tokio::select! {
                biased;
                result = changes.changed() => {
                    if result.is_err() { return; }
                    reason = DelayReason::Change;
                    continue;
                }
                () = tokio::time::sleep(delay) => {}
            }
            reason = DelayReason::Periodic;
            if let Some(download) = self.download() {
                tokio::select! {
                    biased;
                    result = changes.changed() => {
                        if result.is_err() { return; }
                        reason = DelayReason::Change;
                    }
                    result = fetch(&client, &origin, &download) => match result {
                        Ok(Fetched::NotModified) => {},
                        Ok(Fetched::Image(bytes, etag)) => {
                            // Once decoding starts, join it before another download. A cancelled
                            // spawn_blocking task would otherwise keep running without a bound.
                            if let Ok(Ok(png)) = tokio::task::spawn_blocking(move || normalize_image(&bytes)).await {
                                if let Err(error) = self.finish(&download, Some((png, etag))) { tracing::warn!(%error, "Waiting logo cache update failed"); }
                            } else {
                                tracing::warn!("Waiting logo could not be decoded; keeping the previous image");
                            }
                        }
                        Ok(Fetched::Missing) => {
                            if let Err(error) = self.finish(&download, None) { tracing::warn!(%error, "Waiting logo removal failed"); }
                        }
                        Err(reason) => tracing::warn!(reason, "Waiting logo download will retry"),
                    }
                }
            }
        }
    }
}

enum Fetched {
    Image(Vec<u8>, Option<String>),
    NotModified,
    Missing,
}

async fn fetch(
    client: &reqwest::Client,
    origin: &str,
    download: &Download,
) -> Result<Fetched, &'static str> {
    let mut request = client.get(format!(
        "{origin}/api/v2/organizations/{}/logo",
        download.team.organization_id
    ));
    if let Some(etag) = &download.etag {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let mut response = request.send().await.map_err(|_| "request_failed")?;
    match response.status() {
        reqwest::StatusCode::NOT_MODIFIED if download.etag.is_some() => {
            return Ok(Fetched::NotModified);
        }
        reqwest::StatusCode::NOT_FOUND => return Ok(Fetched::Missing),
        reqwest::StatusCode::OK => {}
        _ => return Err("unexpected_http_status"),
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_IMAGE_BYTES as u64)
    {
        return Err("image_too_large");
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 256)
        .map(str::to_owned);
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "body_failed")? {
        if bytes.len().saturating_add(chunk.len()) > MAX_IMAGE_BYTES {
            return Err("image_too_large");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Fetched::Image(bytes, etag))
}

fn normalize_image(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    let format = image::guess_format(bytes).map_err(|_| "unsupported_image")?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return Err("unsupported_image");
    }
    let reader = ImageReader::with_format(Cursor::new(bytes), format);
    let (width, height) = reader.into_dimensions().map_err(|_| "invalid_image")?;
    if width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || u64::from(width) * u64::from(height) > 4_194_304
    {
        return Err("image_dimensions_exceeded");
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|_| "invalid_image")?;
    let mut png = Cursor::new(Vec::new());
    image
        .write_to(&mut png, ImageFormat::Png)
        .map_err(|_| "invalid_image")?;
    Ok(png.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        sync::mpsc,
    };

    fn checked<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        result.unwrap_or_else(|error| panic!("fixture failed: {error:?}"))
    }

    #[test]
    fn delay_samples_cover_endpoints_and_reject_the_incomplete_tail() {
        for (reason, minimum, maximum) in [
            (DelayReason::Initial, 0, 60_000),
            (DelayReason::Periodic, 45_000, 75_000),
            (DelayReason::Change, 0, 5_000),
        ] {
            let buckets = maximum - minimum + 1;
            let sample_space = u64::from(u32::MAX) + 1;
            let limit = sample_space - sample_space % buckets;
            assert_eq!(
                delay_from_sample(reason, 0),
                Some(Duration::from_millis(minimum))
            );
            assert_eq!(
                delay_from_sample(reason, checked(u32::try_from(buckets - 1))),
                Some(Duration::from_millis(maximum))
            );
            assert_eq!(
                delay_from_sample(reason, checked(u32::try_from(limit - 1))),
                Some(Duration::from_millis(maximum))
            );
            for sample in limit..sample_space {
                assert_eq!(
                    delay_from_sample(reason, checked(u32::try_from(sample))),
                    None
                );
            }
            assert_eq!(
                [u32::MAX, 0]
                    .into_iter()
                    .find_map(|sample| delay_from_sample(reason, sample)),
                Some(Duration::from_millis(minimum))
            );
        }
    }

    #[test]
    fn fixed_samples_fill_every_millisecond_bucket_equally() {
        for (reason, minimum, buckets) in [
            (DelayReason::Initial, 0, 60_001_u32),
            (DelayReason::Periodic, 45_000, 30_001),
            (DelayReason::Change, 0, 5_001),
        ] {
            let mut counts = vec![0; checked(usize::try_from(buckets))];
            for sample in 0..buckets * 3 {
                let delay = delay_from_sample(reason, sample)
                    .unwrap_or_else(|| panic!("complete buckets must be accepted"));
                counts[checked(usize::try_from(delay.as_millis() - minimum))] += 1;
            }
            assert!(counts.iter().all(|&count| count == 3), "{reason:?}");
        }
    }

    struct LogoRequest {
        path: String,
        socket: tokio::net::TcpStream,
    }

    impl LogoRequest {
        async fn respond(mut self, image: Option<&[u8]>) {
            let status = if image.is_some() {
                "200 OK"
            } else {
                "404 Not Found"
            };
            let body = image.unwrap_or_default();
            let headers = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            checked(self.socket.write_all(headers.as_bytes()).await);
            checked(self.socket.write_all(body).await);
            checked(self.socket.shutdown().await);
        }
    }

    async fn logo_server() -> (String, mpsc::UnboundedReceiver<LogoRequest>) {
        let listener = checked(tokio::net::TcpListener::bind("127.0.0.1:0").await);
        let origin = format!("http://{}", checked(listener.local_addr()));
        let (sender, requests) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = checked(listener.accept().await);
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(checked(socket.read_u8().await));
                    assert!(request.len() < 4096);
                }
                if sender
                    .send(LogoRequest {
                        path: checked(String::from_utf8(request)),
                        socket,
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
        (origin, requests)
    }

    fn poll_with_delays(
        presentation: &Arc<Presentation>,
        origin: String,
        delays: impl IntoIterator<Item = (DelayReason, u64)>,
    ) -> (
        tokio::task::JoinHandle<()>,
        mpsc::UnboundedReceiver<DelayReason>,
    ) {
        let client = checked(reqwest::Client::builder().no_proxy().build());
        let (sender, reasons) = mpsc::unbounded_channel();
        let mut delays: std::collections::VecDeque<_> = delays.into_iter().collect();
        let polling =
            tokio::spawn(
                Arc::clone(presentation).run_with_delays(client, origin, move |reason| {
                    let (expected, milliseconds) = delays
                        .pop_front()
                        .unwrap_or_else(|| panic!("unexpected delay: {reason:?}"));
                    assert_eq!(reason, expected);
                    checked(sender.send(reason));
                    Duration::from_millis(milliseconds)
                }),
            );
        (polling, reasons)
    }

    async fn next_reason(
        reasons: &mut mpsc::UnboundedReceiver<DelayReason>,
        expected: DelayReason,
    ) {
        assert_eq!(reasons.recv().await, Some(expected));
    }

    async fn before_deadline(
        requests: &mut mpsc::UnboundedReceiver<LogoRequest>,
        milliseconds: u64,
    ) {
        tokio::select! {
            biased;
            _ = requests.recv() => panic!("logo requested before delay elapsed"),
            () = tokio::time::sleep(Duration::from_millis(milliseconds)) => {}
        }
        assert!(requests.try_recv().is_err());
    }

    async fn request_for(
        requests: &mut mpsc::UnboundedReceiver<LogoRequest>,
        organization: &str,
    ) -> LogoRequest {
        let request = requests
            .recv()
            .await
            .unwrap_or_else(|| panic!("logo server stopped"));
        assert!(request.path.starts_with(&format!(
            "GET /api/v2/organizations/{organization}/logo HTTP/1.1\r\n"
        )));
        request
    }

    #[tokio::test(start_paused = true)]
    async fn first_request_and_successive_periods_wait_until_their_deadlines() {
        let (_directory, presentation) = fixture();
        checked(presentation.accept(Some(team())));
        let (origin, mut requests) = logo_server().await;
        let (polling, mut reasons) = poll_with_delays(
            &presentation,
            origin,
            [
                (DelayReason::Initial, 60_000),
                (DelayReason::Periodic, 45_000),
                (DelayReason::Periodic, 75_000),
            ],
        );
        for (reason, milliseconds) in [
            (DelayReason::Initial, 60_000),
            (DelayReason::Periodic, 45_000),
            (DelayReason::Periodic, 75_000),
        ] {
            next_reason(&mut reasons, reason).await;
            before_deadline(&mut requests, milliseconds - 1).await;
            tokio::time::advance(Duration::from_millis(1)).await;
            let request = request_for(&mut requests, "INST-001").await;
            if milliseconds == 75_000 {
                drop(request);
            } else {
                request.respond(None).await;
            }
        }
        polling.abort();
        assert!(polling.await.is_err_and(|error| error.is_cancelled()));
    }

    #[tokio::test(start_paused = true)]
    async fn changes_interrupt_initial_wait_and_redraw_for_only_the_latest_target() {
        let (_directory, presentation) = fixture();
        checked(presentation.accept(Some(team())));
        let (origin, mut requests) = logo_server().await;
        let (polling, mut reasons) = poll_with_delays(
            &presentation,
            origin,
            [
                (DelayReason::Initial, 60_000),
                (DelayReason::Change, 5_000),
                (DelayReason::Change, 2_000),
                (DelayReason::Change, 0),
            ],
        );
        next_reason(&mut reasons, DelayReason::Initial).await;
        before_deadline(&mut requests, 10_000).await;
        for (organization, milliseconds) in [("INST-002", 4_999), ("INST-003", 1_999)] {
            let mut latest = team();
            latest.organization_id = organization.into();
            checked(presentation.accept(Some(latest)));
            next_reason(&mut reasons, DelayReason::Change).await;
            before_deadline(&mut requests, milliseconds).await;
        }
        let mut latest = team();
        latest.organization_id = "INST-004".into();
        checked(presentation.accept(Some(latest)));
        next_reason(&mut reasons, DelayReason::Change).await;
        let _request = request_for(&mut requests, "INST-004").await;
        polling.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn changes_cancel_http_and_delay_the_latest_target_request() {
        let (_directory, presentation) = fixture();
        checked(presentation.accept(Some(team())));
        let (origin, mut requests) = logo_server().await;
        let (polling, mut reasons) = poll_with_delays(
            &presentation,
            origin,
            [
                (DelayReason::Initial, 0),
                (DelayReason::Change, 5_000),
                (DelayReason::Change, 2_000),
            ],
        );
        next_reason(&mut reasons, DelayReason::Initial).await;
        let old = request_for(&mut requests, "INST-001").await;
        for organization in ["INST-002", "INST-003"] {
            let mut latest = team();
            latest.organization_id = organization.into();
            checked(presentation.accept(Some(latest)));
        }
        next_reason(&mut reasons, DelayReason::Change).await;
        before_deadline(&mut requests, 4_999).await;
        let mut latest = team();
        latest.organization_id = "INST-004".into();
        checked(presentation.accept(Some(latest)));
        next_reason(&mut reasons, DelayReason::Change).await;
        before_deadline(&mut requests, 1_999).await;
        tokio::time::advance(Duration::from_millis(1)).await;
        let _request = request_for(&mut requests, "INST-004").await;
        assert!(
            checked(presentation.state.lock())
                .cache
                .logo_digest
                .is_none()
        );
        drop(old);
        polling.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn disconnect_and_unbind_stop_requests_and_reconnect_uses_change_delay() {
        let (_directory, presentation) = fixture();
        checked(presentation.accept(Some(team())));
        let (origin, mut requests) = logo_server().await;
        let (polling, mut reasons) = poll_with_delays(
            &presentation,
            origin,
            [
                (DelayReason::Initial, 0),
                (DelayReason::Change, 1_000),
                (DelayReason::Periodic, 60_000),
                (DelayReason::Change, 2_000),
                (DelayReason::Change, 1_000),
                (DelayReason::Periodic, 60_000),
            ],
        );
        next_reason(&mut reasons, DelayReason::Initial).await;
        let old = request_for(&mut requests, "INST-001").await;
        checked(presentation.disconnect());
        next_reason(&mut reasons, DelayReason::Change).await;
        before_deadline(&mut requests, 1_000).await;
        next_reason(&mut reasons, DelayReason::Periodic).await;
        checked(presentation.accept(Some(team())));
        next_reason(&mut reasons, DelayReason::Change).await;
        before_deadline(&mut requests, 1_999).await;
        tokio::time::advance(Duration::from_millis(1)).await;
        let current = request_for(&mut requests, "INST-001").await;
        checked(presentation.accept(None));
        next_reason(&mut reasons, DelayReason::Change).await;
        before_deadline(&mut requests, 1_000).await;
        next_reason(&mut reasons, DelayReason::Periodic).await;
        before_deadline(&mut requests, 59_999).await;
        assert!(presentation.download().is_none());
        drop((old, current));
        polling.abort();
    }

    #[test]
    fn changes_during_decode_wait_for_completion_and_then_use_change_delay() {
        let (decoder_started, started) = tokio::sync::oneshot::channel();
        let decoder_started = Mutex::new(Some(decoder_started));
        let (release_decoder, decoder_release) = std::sync::mpsc::channel();
        let decoder_release = Mutex::new(decoder_release);
        let runtime = checked(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .start_paused(true)
                .max_blocking_threads(1)
                .on_thread_start(move || {
                    if let Some(started) = checked(decoder_started.lock()).take() {
                        let _ = started.send(());
                        let _ = checked(decoder_release.lock()).recv();
                    }
                })
                .build(),
        );
        runtime.block_on(async move {
            let (_directory, presentation) = fixture();
            checked(presentation.accept(Some(team())));
            let (origin, mut requests) = logo_server().await;
            let (polling, mut reasons) = poll_with_delays(
                &presentation,
                origin,
                [
                    (DelayReason::Initial, 0),
                    (DelayReason::Periodic, 60_000),
                    (DelayReason::Change, 5_000),
                ],
            );
            next_reason(&mut reasons, DelayReason::Initial).await;
            request_for(&mut requests, "INST-001")
                .await
                .respond(Some(&png()))
                .await;
            checked(started.await);
            for organization in ["INST-002", "INST-003"] {
                let mut latest = team();
                latest.organization_id = organization.into();
                checked(presentation.accept(Some(latest)));
            }
            tokio::time::advance(Duration::from_millis(10_000)).await;
            assert!(requests.try_recv().is_err());
            assert!(reasons.try_recv().is_err());
            checked(release_decoder.send(()));
            next_reason(&mut reasons, DelayReason::Periodic).await;
            next_reason(&mut reasons, DelayReason::Change).await;
            assert!(
                checked(presentation.state.lock())
                    .cache
                    .logo_digest
                    .is_none()
            );
            before_deadline(&mut requests, 4_999).await;
            tokio::time::advance(Duration::from_millis(1)).await;
            let _request = request_for(&mut requests, "INST-003").await;
            polling.abort();
        });
    }

    fn team() -> WaitingTeam {
        WaitingTeam {
            binding_id: "01900000-0000-7000-8000-000000000001".into(),
            account_id: "01900000-0000-7000-8000-000000000002".into(),
            seat_code: "A-001".into(),
            organization_id: "INST-001".into(),
            team_name_zh: "夏目与朋友".into(),
            team_name_en: "Natsume and Friends".into(),
            school_name_zh: "示例大学".into(),
            school_name_en: "Example University".into(),
        }
    }

    fn fixture() -> (tempfile::TempDir, Arc<Presentation>) {
        let directory = checked(tempfile::TempDir::new());
        let presentation = restore(&directory, "server-a/device-a");
        (directory, presentation)
    }

    fn restore(directory: &tempfile::TempDir, scope: &str) -> Arc<Presentation> {
        let images = directory.path().join("logos");
        checked(fs::create_dir_all(&images));
        let provider = Arc::new(super::super::binding::tests::input_provider(directory));
        let presentation = Arc::new(Presentation::new(
            directory.path().join("waiting.json"),
            images,
            provider,
        ));
        checked(presentation.configure(scope.into()));
        presentation
    }

    fn png() -> Vec<u8> {
        let mut encoded = Cursor::new(Vec::new());
        checked(image::DynamicImage::new_rgba8(8, 8).write_to(&mut encoded, ImageFormat::Png));
        encoded.into_inner()
    }

    fn install_image(presentation: &Presentation) {
        let request = presentation
            .download()
            .unwrap_or_else(|| panic!("missing request"));
        checked(presentation.finish(&request, Some((png(), Some("\"etag\"".into())))));
    }

    #[test]
    fn restart_restores_text_and_logo_offline_without_any_authority() {
        let (directory, presentation) = fixture();
        checked(presentation.accept(Some(team())));
        install_image(&presentation);
        let restored = restore(&directory, "server-a/device-a");
        let state = checked(restored.state.lock());
        assert!(state.offline);
        assert_eq!(state.cache.team, Some(team()));
        assert!(restored.valid_image(&state.cache));
        drop(state);
        assert!(restored.download().is_none());
        assert!(!directory.path().join("binding-input.json").exists());
    }

    #[test]
    fn unbind_is_durable_and_old_download_cannot_resurrect_the_team() {
        let (directory, presentation) = fixture();
        checked(presentation.accept(Some(team())));
        let old = presentation
            .download()
            .unwrap_or_else(|| panic!("missing request"));
        install_image(&presentation);
        checked(presentation.accept(None));
        checked(presentation.finish(&old, Some((png(), None))));
        checked(presentation.disconnect());
        let restored = restore(&directory, "server-a/device-a");
        let state = checked(restored.state.lock());
        assert!(state.cache.team.is_none());
        assert!(state.cache.logo_digest.is_none());
        assert_eq!(checked(fs::read_dir(&restored.images)).count(), 0);
    }

    #[test]
    fn rebind_uses_new_text_and_default_logo_until_its_own_download_finishes() {
        let (_directory, presentation) = fixture();
        checked(presentation.accept(Some(team())));
        let old = presentation
            .download()
            .unwrap_or_else(|| panic!("missing request"));
        install_image(&presentation);
        let mut replacement = team();
        replacement.binding_id = "01900000-0000-7000-8000-000000000003".into();
        replacement.organization_id = "INST-002".into();
        checked(presentation.accept(Some(replacement.clone())));
        checked(presentation.finish(&old, Some((png(), None))));
        {
            let state = checked(presentation.state.lock());
            assert_eq!(state.cache.team, Some(replacement));
            assert!(state.cache.logo_digest.is_none());
        }
        install_image(&presentation);
        assert!(
            checked(presentation.state.lock())
                .cache
                .logo_digest
                .is_some()
        );
    }

    #[test]
    fn scope_change_discards_old_team_and_unbound_acceptance_overwrites_the_old_scope() {
        let (directory, presentation) = fixture();
        checked(presentation.accept(Some(team())));
        install_image(&presentation);
        for scope in ["server-b/device-a", "server-a/device-b", "other-trust-root"] {
            let restored = restore(&directory, scope);
            assert!(checked(restored.state.lock()).cache.team.is_none());
        }
        let other = restore(&directory, "server-b/device-a");
        checked(other.accept(None));
        let original = restore(&directory, "server-a/device-a");
        assert!(checked(original.state.lock()).cache.team.is_none());
    }

    #[test]
    fn text_update_keeps_current_school_logo_and_disconnect_fences_the_download() {
        let (_directory, presentation) = fixture();
        checked(presentation.accept(Some(team())));
        install_image(&presentation);
        let mut renamed = team();
        renamed.team_name_zh = "更新后的队名".into();
        checked(presentation.accept(Some(renamed.clone())));
        let request = presentation
            .download()
            .unwrap_or_else(|| panic!("missing request"));
        checked(presentation.disconnect());
        checked(presentation.finish(&request, None));
        let state = checked(presentation.state.lock());
        assert!(state.offline);
        assert_eq!(state.cache.team, Some(renamed));
        assert!(state.cache.logo_digest.is_some());
    }

    #[test]
    fn corrupt_image_retains_text_but_bad_metadata_is_not_recovered() {
        let (directory, presentation) = fixture();
        checked(presentation.accept(Some(team())));
        install_image(&presentation);
        let path = presentation
            .image_path(&checked(presentation.state.lock()).cache)
            .unwrap_or_else(|| panic!("missing image"));
        checked(fs::write(path, b"corrupt"));
        let restored = restore(&directory, "server-a/device-a");
        {
            let state = checked(restored.state.lock());
            assert_eq!(state.cache.team, Some(team()));
            assert!(state.cache.logo_digest.is_none());
            assert!(state.cache.etag.is_none());
        }
        checked(fs::write(&restored.record, b"invalid-json"));
        let empty = restore(&directory, "server-a/device-a");
        assert!(checked(empty.state.lock()).cache.team.is_none());
    }

    #[test]
    fn validation_requires_names_and_canonical_school_and_binding_ids() {
        let original = team();
        assert!(valid_team(&original));
        let mut changed = original.clone();
        changed.team_name_zh.clear();
        assert!(valid_team(&changed));
        changed.team_name_en.clear();
        assert!(!valid_team(&changed));
        for school in [
            "../INST-001",
            "INST-1",
            "INST-000",
            "INST-001/other",
            "INST--001",
        ] {
            changed.clone_from(&original);
            changed.organization_id = school.into();
            assert!(!valid_team(&changed));
        }
        changed.clone_from(&original);
        changed.school_name_zh = "school\nname".into();
        assert!(!valid_team(&changed));
    }

    #[test]
    fn image_decode_uses_content_and_rejects_invalid_or_oversized_pixels() {
        let source = png();
        assert_eq!(checked(normalize_image(&source)), source);
        assert!(normalize_image(b"<svg xmlns='http://www.w3.org/2000/svg'/>").is_err());
        assert!(normalize_image(b"corrupt PNG").is_err());
        let mut encoded = Cursor::new(Vec::new());
        checked(image::DynamicImage::new_rgba8(4097, 1).write_to(&mut encoded, ImageFormat::Png));
        assert_eq!(
            normalize_image(&encoded.into_inner()).err(),
            Some("image_dimensions_exceeded")
        );
    }
    #[tokio::test]
    async fn http_logo_reads_png_bytes_then_revalidates_etag_and_handles_removal() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = checked(tokio::net::TcpListener::bind("127.0.0.1:0").await);
        let origin = format!("http://{}", checked(listener.local_addr()));
        let image = png();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for status in ["200 OK", "304 Not Modified", "404 Not Found"] {
                let (mut socket, _) = checked(listener.accept().await);
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(checked(socket.read_u8().await));
                    assert!(request.len() < 4096);
                }
                requests.push(checked(String::from_utf8(request)));
                let body = if status == "200 OK" {
                    image.as_slice()
                } else {
                    &[]
                };
                // Image type is detected from bytes even when metadata claims WebP.
                let headers = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: image/webp\r\nETag: \"logo-v1\"\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                checked(socket.write_all(headers.as_bytes()).await);
                checked(socket.write_all(body).await);
            }
            requests
        });
        let (_directory, presentation) = fixture();
        checked(presentation.accept(Some(team())));
        let client = checked(
            reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build(),
        );
        let mut download = presentation.download().unwrap_or_else(|| panic!("request"));
        let Fetched::Image(png, etag) = checked(fetch(&client, &origin, &download).await) else {
            panic!("expected image");
        };
        assert_eq!(image::guess_format(&png).ok(), Some(ImageFormat::Png));
        assert_eq!(etag.as_deref(), Some("\"logo-v1\""));
        checked(presentation.finish(&download, Some((png, etag))));
        download = presentation.download().unwrap_or_else(|| panic!("request"));
        assert!(matches!(
            checked(fetch(&client, &origin, &download).await),
            Fetched::NotModified
        ));
        assert!(matches!(
            checked(fetch(&client, &origin, &download).await),
            Fetched::Missing
        ));
        checked(presentation.finish(&download, None));
        assert!(
            checked(presentation.state.lock())
                .cache
                .logo_digest
                .is_none()
        );
        let requests = checked(server.await);
        assert!(requests[0].starts_with("GET /api/v2/organizations/INST-001/logo HTTP/1.1"));
        assert!(!requests[0].contains("if-none-match"));
        assert!(requests[1].contains("if-none-match: \"logo-v1\""));
    }
    #[test]
    fn deleted_image_is_fetched_without_etag_and_repaired_at_the_same_digest() {
        use std::os::unix::fs::PermissionsExt as _;
        let (_directory, presentation) = fixture();
        checked(presentation.accept(Some(team())));
        install_image(&presentation);
        let path = presentation
            .image_path(&checked(presentation.state.lock()).cache)
            .unwrap_or_else(|| panic!("image"));
        assert_eq!(
            checked(fs::metadata(&presentation.record))
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            checked(fs::metadata(&path)).permissions().mode() & 0o777,
            0o644
        );
        checked(fs::remove_file(&path));
        assert!(
            presentation
                .download()
                .unwrap_or_else(|| panic!("request"))
                .etag
                .is_none()
        );
        install_image(&presentation);
        assert!(path.is_file());
        assert!(presentation.valid_image(&checked(presentation.state.lock()).cache));
    }
}
