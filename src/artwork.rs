use std::{
    fmt, io,
    io::{Cursor, Read},
    num::NonZeroUsize,
    sync::{Arc, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

use image::{DynamicImage, GenericImageView, ImageReader, Limits, Rgba};
use lru::LruCache;
use ratatui::{Frame, layout::Rect, style::Color};
use ratatui_image::{
    Resize, StatefulImage,
    errors::Errors as RenderError,
    picker::Picker,
    thread::{ResizeRequest, ResizeResponse, ThreadProtocol},
};
use thiserror::Error;

use crate::{app::ArtworkState, config::Theme};

const CACHE_ENTRIES: usize = 8;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_DOWNLOAD_BYTES: usize = 5 * 1024 * 1024;
const MAX_DECODED_EDGE: u32 = 2_048;
const MAX_CACHED_EDGE: u32 = 1_024;
const MAX_DECODED_ALLOCATION: u64 = 32 * 1024 * 1024;

#[derive(Clone, PartialEq)]
pub struct Artwork {
    url: String,
    image: Arc<DynamicImage>,
}

impl Artwork {
    pub(crate) fn new(url: String, image: DynamicImage) -> Self {
        Self {
            url,
            image: Arc::new(image),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn image(&self) -> &DynamicImage {
        &self.image
    }
}

impl fmt::Debug for Artwork {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Artwork")
            .field("url", &self.url)
            .field("dimensions", &self.image.dimensions())
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum ArtworkError {
    #[error("artwork URL must use HTTPS")]
    InsecureUrl,
    #[error("artwork download failed: {0}")]
    Download(String),
    #[error("artwork response was too large (maximum {MAX_DOWNLOAD_BYTES} bytes)")]
    DownloadTooLarge,
    #[error("artwork response has unsupported content type '{0}'")]
    UnsupportedContentType(String),
    #[error("artwork image could not be decoded: {0}")]
    Decode(String),
}

pub trait ArtworkSource {
    fn fetch(&self, url: &str) -> Result<DynamicImage, ArtworkError>;
}

pub struct HttpArtworkSource {
    agent: ureq::Agent,
}

impl Default for HttpArtworkSource {
    fn default() -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .timeout_global(Some(DOWNLOAD_TIMEOUT))
            .user_agent(concat!("spotify-tui/", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl ArtworkSource for HttpArtworkSource {
    fn fetch(&self, url: &str) -> Result<DynamicImage, ArtworkError> {
        if !url.starts_with("https://") {
            return Err(ArtworkError::InsecureUrl);
        }

        let mut response = self
            .agent
            .get(url)
            .call()
            .map_err(|error| ArtworkError::Download(error.to_string()))?;

        if response
            .body()
            .content_length()
            .is_some_and(|length| length > MAX_DOWNLOAD_BYTES as u64)
        {
            return Err(ArtworkError::DownloadTooLarge);
        }

        if let Some(content_type) = response.body().mime_type()
            && !content_type.starts_with("image/")
        {
            return Err(ArtworkError::UnsupportedContentType(
                content_type.to_owned(),
            ));
        }

        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take((MAX_DOWNLOAD_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| ArtworkError::Download(error.to_string()))?;
        if bytes.len() > MAX_DOWNLOAD_BYTES {
            return Err(ArtworkError::DownloadTooLarge);
        }

        decode_artwork(bytes)
    }
}

fn decode_artwork(bytes: Vec<u8>) -> Result<DynamicImage, ArtworkError> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| ArtworkError::Decode(error.to_string()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODED_EDGE);
    limits.max_image_height = Some(MAX_DECODED_EDGE);
    limits.max_alloc = Some(MAX_DECODED_ALLOCATION);
    reader.limits(limits);

    let image = reader
        .decode()
        .map_err(|error| ArtworkError::Decode(error.to_string()))?;
    Ok(
        if image.width() > MAX_CACHED_EDGE || image.height() > MAX_CACHED_EDGE {
            image.thumbnail(MAX_CACHED_EDGE, MAX_CACHED_EDGE)
        } else {
            image
        },
    )
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArtworkEvent {
    Loaded {
        target: ArtworkTarget,
        artwork: Artwork,
    },
    Failed {
        target: ArtworkTarget,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtworkTarget {
    Playback(u64),
    Catalog(u64),
}

enum ArtworkRequest {
    Load { target: ArtworkTarget, url: String },
    Shutdown,
}

pub struct ArtworkRuntime {
    request_tx: mpsc::Sender<ArtworkRequest>,
    event_rx: mpsc::Receiver<ArtworkEvent>,
    worker: Option<JoinHandle<()>>,
}

impl ArtworkRuntime {
    pub fn start() -> io::Result<Self> {
        Self::start_with_source(HttpArtworkSource::default())
    }

    pub fn start_with_source<S>(source: S) -> io::Result<Self>
    where
        S: ArtworkSource + Send + 'static,
    {
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("spotify-tui-artwork".to_owned())
            .spawn(move || run_worker(source, request_rx, &event_tx))?;

        Ok(Self {
            request_tx,
            event_rx,
            worker: Some(worker),
        })
    }

    pub fn load(
        &self,
        target: ArtworkTarget,
        url: impl Into<String>,
    ) -> Result<(), ArtworkRuntimeError> {
        self.request_tx
            .send(ArtworkRequest::Load {
                target,
                url: url.into(),
            })
            .map_err(|_| ArtworkRuntimeError)
    }

    pub fn try_event(&self) -> Option<ArtworkEvent> {
        self.event_rx.try_recv().ok()
    }
}

fn run_worker<S: ArtworkSource>(
    source: S,
    request_rx: mpsc::Receiver<ArtworkRequest>,
    event_tx: &mpsc::Sender<ArtworkEvent>,
) {
    let mut cache = LruCache::new(NonZeroUsize::new(CACHE_ENTRIES).expect("cache is non-empty"));

    while let Ok(request) = request_rx.recv() {
        let mut pending = Vec::with_capacity(2);
        if !queue_latest_by_target(&mut pending, request) {
            return;
        }
        while let Ok(newer) = request_rx.try_recv() {
            if !queue_latest_by_target(&mut pending, newer) {
                return;
            }
        }

        for (target, url) in pending {
            let result = cache.get(&url).cloned().map_or_else(
                || {
                    source.fetch(&url).map(|image| {
                        let artwork = Artwork::new(url.clone(), image);
                        cache.put(url.clone(), artwork.clone());
                        artwork
                    })
                },
                Ok,
            );
            let event = match result {
                Ok(artwork) => ArtworkEvent::Loaded { target, artwork },
                Err(error) => ArtworkEvent::Failed {
                    target,
                    message: error.to_string(),
                },
            };
            if event_tx.send(event).is_err() {
                return;
            }
        }
    }
}

fn queue_latest_by_target(
    pending: &mut Vec<(ArtworkTarget, String)>,
    request: ArtworkRequest,
) -> bool {
    let ArtworkRequest::Load { target, url } = request else {
        return false;
    };
    pending.retain(|(queued, _)| {
        !matches!(
            (queued, target),
            (ArtworkTarget::Playback(_), ArtworkTarget::Playback(_))
                | (ArtworkTarget::Catalog(_), ArtworkTarget::Catalog(_))
        )
    });
    pending.push((target, url));
    true
}

impl Drop for ArtworkRuntime {
    fn drop(&mut self) {
        let _ = self.request_tx.send(ArtworkRequest::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Debug, Error)]
#[error("the artwork runtime has stopped")]
pub struct ArtworkRuntimeError;

pub struct ArtworkRenderer {
    picker: Picker,
    protocol: Option<ThreadProtocol>,
    response_rx: mpsc::Receiver<Result<ResizeResponse, RenderError>>,
    worker: Option<JoinHandle<()>>,
    current_image: Option<(u64, String)>,
}

impl ArtworkRenderer {
    pub fn detect(theme: &Theme) -> io::Result<Self> {
        let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
        Self::new(picker, theme)
    }

    #[cfg(test)]
    pub(crate) fn halfblocks(theme: &Theme) -> io::Result<Self> {
        Self::new(Picker::halfblocks(), theme)
    }

    fn new(mut picker: Picker, theme: &Theme) -> io::Result<Self> {
        picker.set_background_color(Some(background_rgba(theme.background())));
        let (request_tx, request_rx) = mpsc::channel::<ResizeRequest>();
        let (response_tx, response_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("spotify-tui-artwork-render".to_owned())
            .spawn(move || {
                while let Ok(request) = request_rx.recv() {
                    if response_tx.send(request.resize_encode()).is_err() {
                        break;
                    }
                }
            })?;

        Ok(Self {
            picker,
            protocol: Some(ThreadProtocol::new(request_tx, None)),
            response_rx,
            worker: Some(worker),
            current_image: None,
        })
    }

    pub fn sync(&mut self, track_revision: u64, artwork: &ArtworkState) {
        while let Ok(result) = self.response_rx.try_recv() {
            if let Ok(response) = result {
                let _ = self
                    .protocol
                    .as_mut()
                    .expect("renderer protocol should exist")
                    .update_resized_protocol(response);
            }
        }

        match artwork {
            ArtworkState::Ready(artwork) => {
                let identity = (track_revision, artwork.url().to_owned());
                if self.current_image.as_ref() != Some(&identity) {
                    let protocol = self.picker.new_resize_protocol(artwork.image().clone());
                    self.protocol
                        .as_mut()
                        .expect("renderer protocol should exist")
                        .replace_protocol(protocol);
                    self.current_image = Some(identity);
                }
            }
            ArtworkState::Unavailable | ArtworkState::Loading | ArtworkState::Failed(_) => {
                if self.current_image.take().is_some() {
                    self.protocol
                        .as_mut()
                        .expect("renderer protocol should exist")
                        .empty_protocol();
                }
            }
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        frame.render_stateful_widget(
            StatefulImage::new().resize(Resize::Fit(None)),
            area,
            self.protocol
                .as_mut()
                .expect("renderer protocol should exist"),
        );
    }
}

impl Drop for ArtworkRenderer {
    fn drop(&mut self) {
        drop(self.protocol.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

const fn background_rgba(color: Color) -> Rgba<u8> {
    let [red, green, blue] = match color {
        Color::Reset | Color::Black => [0, 0, 0],
        Color::Red => [128, 0, 0],
        Color::Green => [0, 128, 0],
        Color::Yellow => [128, 128, 0],
        Color::Blue => [0, 0, 128],
        Color::Magenta => [128, 0, 128],
        Color::Cyan => [0, 128, 128],
        Color::Gray => [192, 192, 192],
        Color::DarkGray => [128, 128, 128],
        Color::LightRed => [255, 0, 0],
        Color::LightGreen => [0, 255, 0],
        Color::LightYellow => [255, 255, 0],
        Color::LightBlue => [0, 0, 255],
        Color::LightMagenta => [255, 0, 255],
        Color::LightCyan => [0, 255, 255],
        Color::White => [255, 255, 255],
        Color::Rgb(red, green, blue) => [red, green, blue],
        Color::Indexed(_) => [0, 0, 0],
    };
    Rgba([red, green, blue, 255])
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use image::ImageBuffer;
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::config::Config;

    struct CountingSource {
        fetches: Arc<AtomicUsize>,
    }

    impl ArtworkSource for CountingSource {
        fn fetch(&self, _url: &str) -> Result<DynamicImage, ArtworkError> {
            self.fetches.fetch_add(1, Ordering::Relaxed);
            Ok(DynamicImage::new_rgb8(4, 4))
        }
    }

    fn receive_event(runtime: &ArtworkRuntime) -> ArtworkEvent {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(event) = runtime.try_event() {
                return event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for artwork event"
            );
            thread::yield_now();
        }
    }

    #[test]
    fn runtime_reuses_cached_artwork_by_url() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let runtime = ArtworkRuntime::start_with_source(CountingSource {
            fetches: Arc::clone(&fetches),
        })
        .expect("artwork runtime should start");

        runtime
            .load(ArtworkTarget::Playback(1), "https://example.com/art.jpg")
            .expect("first artwork request should be accepted");
        assert!(matches!(
            receive_event(&runtime),
            ArtworkEvent::Loaded {
                target: ArtworkTarget::Playback(1),
                ..
            }
        ));

        runtime
            .load(ArtworkTarget::Catalog(2), "https://example.com/art.jpg")
            .expect("second artwork request should be accepted");
        assert!(matches!(
            receive_event(&runtime),
            ArtworkEvent::Loaded {
                target: ArtworkTarget::Catalog(2),
                ..
            }
        ));
        assert_eq!(fetches.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn pending_requests_keep_the_latest_playback_and_catalogue_images() {
        let mut pending = Vec::new();
        assert!(queue_latest_by_target(
            &mut pending,
            ArtworkRequest::Load {
                target: ArtworkTarget::Playback(1),
                url: "https://example.com/old-playback.jpg".to_owned(),
            },
        ));
        assert!(queue_latest_by_target(
            &mut pending,
            ArtworkRequest::Load {
                target: ArtworkTarget::Catalog(1),
                url: "https://example.com/catalog.jpg".to_owned(),
            },
        ));
        assert!(queue_latest_by_target(
            &mut pending,
            ArtworkRequest::Load {
                target: ArtworkTarget::Playback(2),
                url: "https://example.com/current-playback.jpg".to_owned(),
            },
        ));

        assert_eq!(
            pending,
            vec![
                (
                    ArtworkTarget::Catalog(1),
                    "https://example.com/catalog.jpg".to_owned(),
                ),
                (
                    ArtworkTarget::Playback(2),
                    "https://example.com/current-playback.jpg".to_owned(),
                ),
            ]
        );
    }

    #[test]
    fn invalid_image_data_is_rejected() {
        assert!(matches!(
            decode_artwork(b"not an image".to_vec()),
            Err(ArtworkError::Decode(_))
        ));
    }

    #[test]
    fn http_source_rejects_non_https_urls_before_fetching() {
        assert!(matches!(
            HttpArtworkSource::default().fetch("http://example.com/art.jpg"),
            Err(ArtworkError::InsecureUrl)
        ));
    }

    #[test]
    fn halfblock_renderer_encodes_images_off_the_render_thread() {
        let config = Config::default();
        let mut renderer =
            ArtworkRenderer::halfblocks(config.theme()).expect("renderer should start");
        let artwork = ArtworkState::Ready(Artwork::new(
            "https://example.com/art.png".to_owned(),
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(8, 8, Rgba([255, 0, 0, 255]))),
        ));
        renderer.sync(1, &artwork);
        let mut terminal =
            Terminal::new(TestBackend::new(8, 4)).expect("test backend is infallible");
        terminal
            .draw(|frame| renderer.render(frame, frame.area()))
            .expect("test backend is infallible");

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            renderer.sync(1, &artwork);
            terminal
                .draw(|frame| renderer.render(frame, frame.area()))
                .expect("test backend is infallible");
            let rendered = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .any(|cell| cell.symbol() == "▀");
            if rendered {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for the half-block render"
            );
            thread::yield_now();
        }
    }
}
