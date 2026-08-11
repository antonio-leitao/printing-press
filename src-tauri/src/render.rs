//! Native page rasterisation and text extraction, on MuPDF.
//!
//! MuPDF's `Document` and `Page` are `!Send`: a context belongs to the thread
//! that made it. Everything here therefore runs on one owning thread, and the
//! rest of the application talks to it over a channel. That is also what keeps
//! rasterisation off the UI thread, which is the whole point.

use std::path::{Path, PathBuf};

use mupdf::{Colorspace, Document, Matrix, TextExtractOptions, TextPageFlags};

use crate::error::{AppError, AppResult};

/// Words carry enough structure for a selection overlay without one DOM node
/// per glyph.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    /// In PDF points, with the origin at the top left of the page.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageGeometry {
    pub width: f32,
    pub height: f32,
}

/// A rasterised page as tightly packed RGBA, four bytes per pixel.
///
/// MuPDF draws RGB with an opaque white background, which is what a page should
/// look like. The alpha channel is added here rather than in the webview: the
/// browser needs RGBA for `ImageData`, and expanding three million pixels in
/// JavaScript costs more than the render itself.
///
/// The buffer opens with [`RenderedPage::PREFIX`] free bytes. Whoever ships the
/// page writes its dimensions there; see [`into_framed`](Self::into_framed).
pub struct RenderedPage {
    pub width: u32,
    pub height: u32,
    buffer: Vec<u8>,
}

impl RenderedPage {
    /// Room kept at the front of the buffer for the dimensions the webview reads
    /// before the samples.
    ///
    /// A page is twelve megabytes at ordinary zoom on a retina display. Building
    /// the response by allocating a second buffer and copying the samples into
    /// it behind a header cost as much as a tenth of the render that produced
    /// them, for nothing: the space can simply be left at the front to begin
    /// with, and the header written into it.
    pub const PREFIX: usize = 8;

    /// The samples alone, without the space kept in front of them.
    ///
    /// For tests. Everything on the way to the webview wants the whole buffer,
    /// which is the point of the prefix.
    #[cfg(test)]
    pub fn samples(&self) -> &[u8] {
        &self.buffer[Self::PREFIX..]
    }

    fn samples_mut(&mut self) -> &mut [u8] {
        &mut self.buffer[Self::PREFIX..]
    }

    /// The whole buffer, with `prefix` written into the space kept for it. No
    /// copy: this is the same allocation MuPDF's samples were expanded into.
    pub fn into_framed(mut self, prefix: [u8; Self::PREFIX]) -> Vec<u8> {
        self.buffer[..Self::PREFIX].copy_from_slice(&prefix);
        self.buffer
    }
}

/// A link on a page: where it is, and where it goes.
///
/// A PDF makes no distinction beyond a URI string — `#page=4` and
/// `https://…` arrive the same way — so MuPDF's own resolution is what
/// separates the two here. A reference or a citation resolves to a place in
/// this document; anything else is somewhere outside it.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    /// In PDF points, with the origin at the top left of the page.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// The page this leads to, 1-based, when it leads inside the document.
    pub page: Option<usize>,
    /// How far down that page, in PDF points, when the destination says.
    pub top: Option<f32>,
    /// Where it leads outside the document.
    pub uri: Option<String>,
}

/// A search hit, as a rectangle in PDF points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub page: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

fn mupdf_error(context: &str, error: mupdf::Error) -> AppError {
    AppError::Build(format!("{context}: {error}"))
}

pub fn open(path: &Path) -> AppResult<Document> {
    let path = path
        .to_str()
        .ok_or_else(|| AppError::InvalidInput("PDF path is not valid UTF-8".into()))?;
    Document::open(path).map_err(|error| mupdf_error("could not open the PDF", error))
}

pub fn page_count(document: &Document) -> AppResult<usize> {
    document
        .page_count()
        .map(|count| count.max(0) as usize)
        .map_err(|error| mupdf_error("could not count pages", error))
}

/// Every page's size, so the viewer can lay out the whole document before a
/// single page has been drawn.
pub fn geometry(document: &Document) -> AppResult<Vec<PageGeometry>> {
    let count = page_count(document)?;
    let mut pages = Vec::with_capacity(count);
    for index in 0..count {
        let page = document
            .load_page(index as i32)
            .map_err(|error| mupdf_error("could not load a page", error))?;
        let bounds = page
            .bounds()
            .map_err(|error| mupdf_error("could not measure a page", error))?;
        pages.push(PageGeometry {
            width: bounds.x1 - bounds.x0,
            height: bounds.y1 - bounds.y0,
        });
    }
    Ok(pages)
}

/// Rasterises one page. `scale` is device pixels per PDF point, so it already
/// carries the display's pixel ratio.
pub fn render_page(
    document: &Document,
    index: usize,
    scale: f32,
    invert: bool,
) -> AppResult<RenderedPage> {
    let page = document
        .load_page(index as i32)
        .map_err(|error| mupdf_error("could not load the page", error))?;
    let matrix = Matrix::new_scale(scale, scale);
    // alpha = false so the page comes back on opaque white rather than
    // transparent, which is what a sheet of paper looks like.
    let pixmap = page
        .to_pixmap(&matrix, &Colorspace::device_rgb(), false, true)
        .map_err(|error| mupdf_error("could not rasterise the page", error))?;
    let mut page = RenderedPage {
        width: pixmap.width(),
        height: pixmap.height(),
        buffer: to_rgba(pixmap.samples(), pixmap.n()),
    };
    if invert {
        darken(page.samples_mut());
    }
    Ok(page)
}

/// Expands MuPDF's samples to RGBA, behind the space [`RenderedPage::PREFIX`]
/// keeps for the header.
fn to_rgba(samples: &[u8], components: u8) -> Vec<u8> {
    let pixels = match components {
        4 => samples.len() / 4,
        3 => samples.len() / 3,
        1 => samples.len(),
        _ => 0,
    };
    let mut rgba = vec![0_u8; RenderedPage::PREFIX];
    rgba.reserve_exact(pixels * 4);
    match components {
        4 => rgba.extend_from_slice(samples),
        3 => {
            for pixel in samples.chunks_exact(3) {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        1 => {
            for &grey in samples {
                rgba.extend_from_slice(&[grey, grey, grey, 255]);
            }
        }
        // Nothing this can be read as. The header still travels, and the
        // webview refuses a body that does not match the dimensions in it.
        _ => {}
    }
    rgba
}

/// The two colours an inverted page is drawn between: what paper becomes, and
/// what ink becomes. They are Fandango's `lines` and `text` — the same palette
/// the interface is dressed in — and `--sheet-inverted` in `app.css` is the
/// first of them, so the sheet a page rests on and the paper drawn onto it are
/// one colour.
///
/// Paper is not the darkest colour in the palette, and that is the point: the
/// ground the viewer lays a page on is, and a page has to be brighter than the
/// ground under it to read as a page. It is the same relation the light theme
/// has, where paper is white and the ground beside it is not quite.
///
/// It is a quieter relation than the light one, though — a shade narrower, and
/// without the shadow that also picks a page out in the light, since a contact
/// shadow on a ground this dark has almost nothing to fall on. That is the
/// intention: a page read in the dark should sit in the room rather than
/// announce itself.
///
/// Ink stops well short of white for its own reason: maximum contrast over a
/// page's worth of text is what makes reading in the dark tiring.
const INVERTED_PAPER: [u8; 3] = [0x0e, 0x11, 0x14];
const INVERTED_INK: [u8; 3] = [0xbb, 0xbb, 0xbb];

/// Turns a page inside out for reading in the dark: paper goes to near black,
/// ink comes back a light grey, and everything with a colour keeps it.
///
/// Only the brightness is flipped. Each pixel's luminance is inverted the way a
/// grey would be — through sRGB's own curve, so a midtone stays a midtone —
/// and the result is then read off the ramp between the two ends above. A grey
/// takes that ramp exactly, which is what puts paper and ink precisely on their
/// colours. Anything with a hue is instead scaled to the ramp's brightness with
/// its channel ratios untouched, so a red curve comes back red rather than
/// cyan, which is where a per-channel inversion sends it; MuPDF's own `invert`
/// and `tint` are both per-channel. The two are blended by how much colour the
/// pixel actually had, so there is no seam between a grey and a near-grey.
///
/// Two honest costs. A light saturated colour has to come back dark to have
/// been inverted at all, so yellow reads as olive. And this no longer undoes
/// itself: the ramp is narrower than the range it maps from, so a page put
/// through it twice comes back flatter. Nothing does that — inversion is a way
/// of drawing the original page, never of drawing an inverted one.
fn darken(samples: &mut [u8]) {
    for pixel in samples.chunks_exact_mut(4) {
        let red = LINEAR[pixel[0] as usize];
        let green = LINEAR[pixel[1] as usize];
        let blue = LINEAR[pixel[2] as usize];

        let luminance = 0.2126 * red + 0.7152 * green + 0.0722 * blue;
        // Flipped as an 8-bit grey rather than in linear light: sRGB's curve is
        // roughly how brightness is seen, and inverting there is what makes the
        // flip symmetric instead of dragging every midtone down.
        let flipped = 255 - encoded(luminance);
        let (ground, target) = RAMP[flipped as usize];

        // How much colour there is to keep. Measured against the brightest
        // channel, so it is a proportion rather than an amount: a dark red is
        // as red as a light one.
        let high = red.max(green).max(blue);
        let low = red.min(green).min(blue);
        let chroma = if high <= f32::EPSILON {
            0.0
        } else {
            (high - low) / high
        };
        if chroma <= f32::EPSILON {
            // A grey, which is most of a page. Straight onto the ramp.
            pixel[..3].copy_from_slice(&ground);
            continue;
        }

        // `high` is above epsilon here, so the luminance cannot be zero.
        let factor = target / luminance;
        for (channel, value) in [red, green, blue].into_iter().enumerate() {
            let scaled = encoded(value * factor);
            pixel[channel] = mix(ground[channel], scaled, chroma);
        }
    }
}

/// Blends `from` toward `to`, with `amount` in 0..=1.
fn mix(from: u8, to: u8, amount: f32) -> u8 {
    let from = f32::from(from);
    (from + (f32::from(to) - from) * amount).round() as u8
}

/// The ramp an inverted page is drawn on, and the linear luminance of each of
/// its steps. Both are read once per pixel, so both are counted out in advance
/// rather than worked out a few million times.
static RAMP: std::sync::LazyLock<[([u8; 3], f32); 256]> = std::sync::LazyLock::new(|| {
    std::array::from_fn(|index| {
        let amount = index as f32 / 255.0;
        let step: [u8; 3] = std::array::from_fn(|channel| {
            mix(INVERTED_PAPER[channel], INVERTED_INK[channel], amount)
        });
        let luminance = 0.2126 * LINEAR[step[0] as usize]
            + 0.7152 * LINEAR[step[1] as usize]
            + 0.0722 * LINEAR[step[2] as usize];
        (step, luminance)
    })
});

/// sRGB's transfer curve, both ways, as tables. A page is millions of pixels
/// and this runs on every one of them, so neither direction can afford `powf`.
static LINEAR: std::sync::LazyLock<[f32; 256]> = std::sync::LazyLock::new(|| {
    std::array::from_fn(|index| {
        let value = index as f32 / 255.0;
        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    })
});

/// Linear light back to a channel. Indexed rather than solved: the step is fine
/// enough that no value lands more than one level from where the curve puts it.
static ENCODED: std::sync::LazyLock<[u8; ENCODE_STEPS]> = std::sync::LazyLock::new(|| {
    std::array::from_fn(|index| {
        let value = index as f32 / (ENCODE_STEPS - 1) as f32;
        let encoded = if value <= 0.003_130_8 {
            value * 12.92
        } else {
            1.055 * value.powf(1.0 / 2.4) - 0.055
        };
        (encoded * 255.0).round() as u8
    })
});

const ENCODE_STEPS: usize = 4096;

fn encoded(value: f32) -> u8 {
    let index = (value.clamp(0.0, 1.0) * (ENCODE_STEPS - 1) as f32).round() as usize;
    ENCODED[index]
}

/// Word boxes for the selection overlay, in PDF points.
pub fn words(document: &Document, index: usize) -> AppResult<Vec<Word>> {
    let page = document
        .load_page(index as i32)
        .map_err(|error| mupdf_error("could not load the page", error))?;
    let extracted = page
        .words(TextExtractOptions {
            flags: TextPageFlags::PRESERVE_WHITESPACE,
        })
        .map_err(|error| mupdf_error("could not extract text", error))?;
    Ok(extracted
        .into_iter()
        .map(|word| Word {
            text: word.text,
            x: word.bounds.x0,
            y: word.bounds.y0,
            width: word.bounds.x1 - word.bounds.x0,
            height: word.bounds.y1 - word.bounds.y0,
            line: word.line,
        })
        .collect())
}

/// The links on one page, in PDF points.
pub fn links(document: &Document, index: usize) -> AppResult<Vec<Link>> {
    let page = document
        .load_page(index as i32)
        .map_err(|error| mupdf_error("could not load the page", error))?;
    let found = page
        .links()
        .map_err(|error| mupdf_error("could not read the page's links", error))?;
    Ok(found
        .map(|link| {
            let bounds = link.bounds;
            Link {
                x: bounds.x0,
                y: bounds.y0,
                width: bounds.x1 - bounds.x0,
                height: bounds.y1 - bounds.y0,
                // A resolved destination is what makes it internal. MuPDF has
                // already turned the named destination into a page number.
                page: link.dest.map(|dest| dest.loc.page_number as usize + 1),
                top: link.dest.and_then(|dest| destination_top(dest.kind)),
                uri: link.dest.is_none().then(|| link.uri.clone()),
            }
        })
        .collect())
}

/// How far down the target page a destination asks for, when it asks at all.
/// The kinds that fit a page to the window have no vertical position of their
/// own, and a link into the top of a page is the sensible reading of those.
fn destination_top(kind: mupdf::DestinationKind) -> Option<f32> {
    use mupdf::DestinationKind::{FitBH, FitH, FitR, XYZ};
    match kind {
        XYZ { top, .. } | FitH { top } | FitBH { top } => top,
        FitR { top, .. } => Some(top),
        _ => None,
    }
}

const MAX_HITS_PER_PAGE: u32 = 500;

/// MuPDF's own search. This is the same routine zathura uses.
pub fn search_page(document: &Document, index: usize, needle: &str) -> AppResult<Vec<Hit>> {
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let page = document
        .load_page(index as i32)
        .map_err(|error| mupdf_error("could not load the page", error))?;
    let quads = page
        .search(needle, MAX_HITS_PER_PAGE)
        .map_err(|error| mupdf_error("could not search the page", error))?;
    Ok(quads
        .iter()
        .map(|quad| {
            // A quad is four corners; for a highlight the enclosing box is enough.
            let left = quad.ul.x.min(quad.ll.x);
            let right = quad.ur.x.max(quad.lr.x);
            let top = quad.ul.y.min(quad.ur.y);
            let bottom = quad.ll.y.max(quad.lr.y);
            Hit {
                page: index,
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            }
        })
        .collect())
}

// -- the pool -------------------------------------------------------------

use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
};

use tokio::sync::oneshot;

/// Documents kept open per worker. Reopening costs about 4ms, so a handful is
/// plenty to keep scrolling and a side-by-side comparison warm.
const DOCUMENTS_PER_WORKER: usize = 4;

/// How many pages may be waiting to be drawn before the oldest is given up on.
///
/// Far more than can be near the window at once, which is what makes dropping
/// safe: for a page to be given up on, this many *newer* pages must have been
/// asked for since, and a reader that has moved that far has moved off it.
const MOST_PENDING_RENDERS: usize = 32;

pub enum Job {
    Render {
        path: PathBuf,
        page: usize,
        /// Device pixels per PDF point.
        scale: f32,
        /// Drawn for a dark room: see `darken`.
        invert: bool,
        reply: oneshot::Sender<AppResult<RenderedPage>>,
    },
    Geometry {
        path: PathBuf,
        reply: oneshot::Sender<AppResult<Vec<PageGeometry>>>,
    },
    Words {
        path: PathBuf,
        page: usize,
        reply: oneshot::Sender<AppResult<Vec<Word>>>,
    },
    Links {
        path: PathBuf,
        page: usize,
        reply: oneshot::Sender<AppResult<Vec<Link>>>,
    },
    Search {
        path: PathBuf,
        needle: String,
        reply: oneshot::Sender<AppResult<Vec<Hit>>>,
    },
}

impl Job {
    /// True when whoever asked has gone away.
    ///
    /// This catches a caller whose own future was dropped. It does *not* catch
    /// the webview abandoning a page: those requests are served from a task
    /// Tauri spawns for the `press:` scheme, and an aborted `fetch` never
    /// reaches it. Which is why the queue below is ordered the way it is.
    fn abandoned(&self) -> bool {
        match self {
            Self::Render { reply, .. } => reply.is_closed(),
            Self::Geometry { reply, .. } => reply.is_closed(),
            Self::Words { reply, .. } => reply.is_closed(),
            Self::Links { reply, .. } => reply.is_closed(),
            Self::Search { reply, .. } => reply.is_closed(),
        }
    }

    fn is_render(&self) -> bool {
        matches!(self, Self::Render { .. })
    }
}

/// What the workers take from, newest first.
///
/// A page is asked for when it comes near the window, and nothing ever un-asks
/// it — see [`Job::abandoned`]. Drawn oldest-first, a fast scroll through a long
/// document therefore leaves the page under the reader's eyes waiting behind
/// every page they have already scrolled past: a hundred of them, at about ten
/// milliseconds each.
///
/// Newest-first puts what is on screen at the front. The pages scrolled past are
/// still drawn, harmlessly and last, unless more than [`MOST_PENDING_RENDERS`]
/// are waiting — at which point the oldest is given up on, because by then it is
/// certainly not being looked at.
#[derive(Default)]
struct Queue {
    pending: Mutex<Pending>,
    ready: Condvar,
}

#[derive(Default)]
struct Pending {
    jobs: VecDeque<Job>,
    closed: bool,
}

impl Queue {
    fn push(&self, job: Job) -> bool {
        let Ok(mut pending) = self.pending.lock() else {
            return false;
        };
        if pending.closed {
            return false;
        }
        pending.jobs.push_back(job);
        // Only pages are given up on. Geometry, links, words and search are each
        // asked for once and waited on, so dropping one would fail something
        // nobody has walked away from.
        while pending.jobs.iter().filter(|job| job.is_render()).count() > MOST_PENDING_RENDERS {
            let Some(oldest) = pending.jobs.iter().position(Job::is_render) else {
                break;
            };
            pending.jobs.remove(oldest);
        }
        drop(pending);
        self.ready.notify_one();
        true
    }

    fn take(&self) -> Option<Job> {
        let mut pending = self.pending.lock().ok()?;
        loop {
            if let Some(job) = pending.jobs.pop_back() {
                return Some(job);
            }
            if pending.closed {
                return None;
            }
            pending = self.ready.wait(pending).ok()?;
        }
    }

    fn close(&self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.closed = true;
            pending.jobs.clear();
        }
        self.ready.notify_all();
    }
}

/// A few threads, each owning its own MuPDF context and open documents.
pub struct RenderPool {
    queue: Arc<Queue>,
}

impl RenderPool {
    pub fn new(workers: usize) -> Self {
        let queue = Arc::new(Queue::default());
        for index in 0..workers.max(1) {
            let queue = Arc::clone(&queue);
            std::thread::Builder::new()
                .name(format!("press-render-{index}"))
                .spawn(move || worker(&queue))
                .expect("could not start a render thread");
        }
        Self { queue }
    }

    /// Sized to keep a couple of pages in flight without starving the rest of
    /// the machine during a build.
    pub fn with_default_size() -> Self {
        let parallelism = std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(4);
        Self::new(parallelism.saturating_sub(1).clamp(2, 4))
    }

    async fn submit<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<AppResult<T>>) -> Job,
    ) -> AppResult<T> {
        let (reply, receive) = oneshot::channel();
        if !self.queue.push(make(reply)) {
            return Err(AppError::Task("the render pool has stopped".into()));
        }
        receive
            .await
            .map_err(|_| AppError::Task("the render pool dropped a request".into()))?
    }

    pub async fn render(
        &self,
        path: PathBuf,
        page: usize,
        scale: f32,
        invert: bool,
    ) -> AppResult<RenderedPage> {
        self.submit(|reply| Job::Render {
            path,
            page,
            scale,
            invert,
            reply,
        })
        .await
    }

    pub async fn geometry(&self, path: PathBuf) -> AppResult<Vec<PageGeometry>> {
        self.submit(|reply| Job::Geometry { path, reply }).await
    }

    pub async fn words(&self, path: PathBuf, page: usize) -> AppResult<Vec<Word>> {
        self.submit(|reply| Job::Words { path, page, reply }).await
    }

    pub async fn links(&self, path: PathBuf, page: usize) -> AppResult<Vec<Link>> {
        self.submit(|reply| Job::Links { path, page, reply }).await
    }

    pub async fn search(&self, path: PathBuf, needle: String) -> AppResult<Vec<Hit>> {
        self.submit(|reply| Job::Search {
            path,
            needle,
            reply,
        })
        .await
    }
}

/// Lets the workers finish. Only tests drop a pool — the application's lives as
/// long as it does — but a thread per pool left parked on the condition variable
/// would accumulate through a test run.
impl Drop for RenderPool {
    fn drop(&mut self) {
        self.queue.close();
    }
}

/// What makes a cached document still the document on disk.
///
/// A path alone cannot say. Press's own builds publish under a fresh
/// `build-<stamp>.pdf` every time, so for those the path is enough — but a PDF
/// Press is only showing is rebuilt under its own name by whatever owns it, and
/// that is the whole point of watching one. MuPDF reads objects from the stream
/// as they are asked for, so a handle held across a rewrite resolves the new
/// bytes through the old cross-reference table: not a stale page, a wrong one.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<std::time::SystemTime>,
    len: u64,
}

impl Stamp {
    /// A file that cannot be measured gets a stamp that equals nothing, itself
    /// included, so it is reopened rather than trusted.
    fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        })
    }
}

struct Entry {
    path: PathBuf,
    stamp: Option<Stamp>,
    document: Document,
}

/// Open documents for one thread. MuPDF's context belongs to the thread that
/// made it, so this is deliberately not shared.
struct DocumentCache {
    entries: VecDeque<Entry>,
}

impl DocumentCache {
    fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    fn get(&mut self, path: &Path) -> AppResult<&Document> {
        // One `stat` against a render measured in milliseconds.
        let stamp = Stamp::of(path);
        if let Some(position) = self
            .entries
            .iter()
            .position(|entry| entry.path.as_path() == path)
        {
            let fresh = stamp.is_some_and(|stamp| self.entries[position].stamp == Some(stamp));
            let entry = self.entries.remove(position).expect("position is in range");
            if fresh {
                self.entries.push_back(entry);
                return Ok(&self.entries.back().expect("just moved").document);
            }
            // Rewritten under the same name: what is open describes the file
            // that used to be there. Dropped, and opened again below.
        }

        let document = open(path)?;
        self.entries.push_back(Entry {
            path: path.to_path_buf(),
            stamp,
            document,
        });
        while self.entries.len() > DOCUMENTS_PER_WORKER {
            self.entries.pop_front();
        }
        Ok(&self.entries.back().expect("just inserted").document)
    }
}

fn worker(queue: &Queue) {
    let mut cache = DocumentCache::new();
    // The queue's lock is held only to take a job, never across the work itself.
    while let Some(job) = queue.take() {
        if job.abandoned() {
            continue;
        }

        match job {
            Job::Render {
                path,
                page,
                scale,
                invert,
                reply,
            } => {
                let result = cache
                    .get(&path)
                    .and_then(|document| render_page(document, page, scale, invert));
                let _ = reply.send(result);
            }
            Job::Geometry { path, reply } => {
                let result = cache.get(&path).and_then(geometry);
                let _ = reply.send(result);
            }
            Job::Words { path, page, reply } => {
                let result = cache.get(&path).and_then(|document| words(document, page));
                let _ = reply.send(result);
            }
            Job::Links { path, page, reply } => {
                let result = cache.get(&path).and_then(|document| links(document, page));
                let _ = reply.send(result);
            }
            Job::Search {
                path,
                needle,
                reply,
            } => {
                let result = cache.get(&path).and_then(|document| {
                    let count = page_count(document)?;
                    let mut hits = Vec::new();
                    for index in 0..count {
                        hits.extend(search_page(document, index, &needle)?);
                    }
                    Ok(hits)
                });
                let _ = reply.send(result);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn inverted(red: u8, green: u8, blue: u8) -> [u8; 3] {
        let mut pixel = [red, green, blue, 255];
        darken(&mut pixel);
        [pixel[0], pixel[1], pixel[2]]
    }

    /// The whole reason this is not a channel inversion: a page has to survive
    /// being read in the dark, figures included.
    #[test]
    fn inverting_a_page_flips_lightness_and_leaves_colour_alone() {
        // Paper and ink trade places, landing exactly on the two ends of the
        // ramp — the same near black the interface uses, and a grey short of
        // white. Exactly, because paper meets `--sheet-inverted` at the edge of
        // the page and a near miss there would show as a seam.
        assert_eq!(inverted(255, 255, 255), INVERTED_PAPER);
        assert_eq!(inverted(0, 0, 0), INVERTED_INK);

        // A red stays red. Inverting the channels would send it to cyan, which
        // is what MuPDF's own invert and tint both do.
        let red = inverted(220, 40, 40);
        assert!(
            red[0] > red[1] + 40 && red[0] > red[2] + 40,
            "still recognisably red: {red:?}"
        );
        let blue = inverted(40, 70, 200);
        assert!(
            blue[2] > blue[0] + 40 && blue[2] > blue[1] + 40,
            "still recognisably blue: {blue:?}"
        );

        // Lightness really is flipped, not merely nudged: what was dark comes
        // back light.
        let was_dark = inverted(30, 60, 30);
        let was_light = inverted(200, 240, 200);
        assert!(
            luma(was_dark) > luma(was_light) + 60.0,
            "a dark green and a light one swap: {was_dark:?} {was_light:?}"
        );

        // Mid grey lands mid ramp, which is what makes the flip symmetric
        // rather than a darkening. Mid ramp, not mid range: the ramp no longer
        // reaches either end of what a screen can show.
        let grey = inverted(128, 128, 128);
        let middle = RAMP[127].0;
        for (channel, expected) in grey.iter().zip(middle.iter()) {
            assert!(
                channel.abs_diff(*expected) <= 2,
                "mid grey stays mid: {grey:?} against {middle:?}"
            );
        }
    }

    /// The sheet a page rests on is set in CSS and the paper drawn onto it is
    /// set here, and the two have to be one colour. The drawing covers the
    /// sheet exactly, so a difference does not show as a wrong page — it shows
    /// as a seam at the edge, and only for as long as it takes the bitmap to
    /// arrive, which is the kind of thing that survives a lot of looking at.
    #[test]
    fn the_stylesheet_agrees_on_what_paper_becomes() {
        let stylesheet = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/app.css");
        let css = std::fs::read_to_string(&stylesheet)
            .unwrap_or_else(|error| panic!("{}: {error}", stylesheet.display()));
        let declared = css
            .lines()
            .find_map(|line| line.trim().strip_prefix("--sheet-inverted:"))
            .map(|value| value.trim().trim_end_matches(';').to_lowercase())
            .expect("app.css declares --sheet-inverted");
        let [red, green, blue] = INVERTED_PAPER;
        assert_eq!(
            declared,
            format!("#{red:02x}{green:02x}{blue:02x}"),
            "--sheet-inverted and INVERTED_PAPER have to be the same colour"
        );
    }

    /// Every grey on the page lands on the ramp between the two ends, in order.
    /// Greys are almost all of a page — the text, the rules, the margins — so
    /// this is the property that decides what reading in the dark looks like.
    #[test]
    fn greys_land_on_the_ramp_in_order() {
        let mut previous: Option<[u8; 3]> = None;
        for level in (0..=255).rev() {
            let step = inverted(level, level, level);
            // On the ramp: a grey has no colour to keep, so it takes it exactly.
            assert!(
                RAMP.iter().any(|(colour, _)| *colour == step),
                "grey {level} landed off the ramp at {step:?}"
            );
            // Darker source, lighter result — the flip, with no reversals.
            if let Some(previous) = previous {
                assert!(
                    luma(step) >= luma(previous),
                    "grey {level} broke the order: {step:?} after {previous:?}"
                );
            }
            previous = Some(step);
        }
    }

    /// Nothing inverts an already inverted page — the flag picks how the
    /// original is drawn — but the ramp being narrower than the range it maps
    /// from is worth pinning down, because it is the cost of not using pure
    /// black and white and the reason the round trip is gone.
    #[test]
    fn the_ramp_is_narrower_than_the_page_it_maps_from() {
        let paper = inverted(255, 255, 255);
        let ink = inverted(0, 0, 0);
        assert!(
            luma(ink) - luma(paper) < 255.0 * 0.75,
            "the ramp is deliberately short of the full range: {paper:?} to {ink:?}"
        );
        assert!(
            luma(ink) - luma(paper) > 255.0 * 0.5,
            "but still most of it, or the page would be flat: {paper:?} to {ink:?}"
        );
    }

    fn luma(pixel: [u8; 3]) -> f32 {
        0.2126 * f32::from(pixel[0]) + 0.7152 * f32::from(pixel[1]) + 0.0722 * f32::from(pixel[2])
    }

    /// Builds a small real PDF with the installed TeX, so the test exercises the
    /// kind of output Press actually shows.
    fn fixture() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
        let latexmk = crate::toolchain::resolve_executable("latexmk")?;
        let directory = tempfile::tempdir().ok()?;
        let source = directory.path().join("main.tex");
        std::fs::write(
            &source,
            "\\documentclass{article}\n\\begin{document}\n\
             \\section{Introduction}\nPress renders this natively.\n\
             \\newpage\nA second page mentioning kestrel once.\n\
             \\end{document}\n",
        )
        .ok()?;
        let status = std::process::Command::new(latexmk)
            .args(["-pdf", "-interaction=nonstopmode"])
            .arg(format!("-outdir={}", directory.path().display()))
            .arg(&source)
            .current_dir(directory.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()?;
        if !status.success() {
            return None;
        }
        let pdf = directory.path().join("main.pdf");
        pdf.is_file().then_some((directory, pdf))
    }

    /// A document with the two kinds of link a paper has: a cross-reference
    /// that leads to another page of itself, and an address that leads out.
    fn linked_fixture() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
        let latexmk = crate::toolchain::resolve_executable("latexmk")?;
        let directory = tempfile::tempdir().ok()?;
        let source = directory.path().join("linked.tex");
        std::fs::write(
            &source,
            "\\documentclass{article}\n\\usepackage{hyperref}\n\\begin{document}\n\
             See \\autoref{later} and \\url{https://example.org/paper}.\n\
             \\newpage\n\\section{Later}\\label{later}\nThe target.\n\
             \\end{document}\n",
        )
        .ok()?;
        let status = std::process::Command::new(latexmk)
            .args(["-pdf", "-interaction=nonstopmode"])
            .arg(format!("-outdir={}", directory.path().display()))
            .arg(&source)
            .current_dir(directory.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()?;
        if !status.success() {
            return None;
        }
        let pdf = directory.path().join("linked.pdf");
        pdf.is_file().then_some((directory, pdf))
    }

    /// Both kinds come back, told apart by whether MuPDF could resolve them,
    /// and positioned in the same top-left space as everything else the viewer
    /// is given.
    #[test]
    fn reads_both_kinds_of_link_off_a_real_page() {
        let Some((_guard, pdf)) = linked_fixture() else {
            eprintln!("skipping: latexmk is not installed");
            return;
        };
        let document = open(&pdf).unwrap();
        let page = geometry(&document).unwrap()[0];
        let found = links(&document, 0).unwrap();
        assert!(found.len() >= 2, "a reference and an address: {found:?}");

        let outward = found
            .iter()
            .find(|link| link.uri.is_some())
            .expect("the url is a link out of the document");
        assert_eq!(outward.uri.as_deref(), Some("https://example.org/paper"));
        assert!(outward.page.is_none(), "an address has no page of its own");

        let inward = found
            .iter()
            .find(|link| link.page.is_some())
            .expect("the cross-reference is a link within the document");
        assert_eq!(inward.page, Some(2), "\\autoref points at the second page");
        assert!(
            inward.uri.is_none(),
            "a destination inside is not an address"
        );
        // The section it points at is near the top of its page. Measured from
        // the top this is a small number; measured the way PDF itself does it,
        // from the bottom, it would be most of a page height — and every jump
        // would land a page away from where it was aimed.
        let top = inward.top.expect("hyperref names a point, not just a page");
        assert!(
            top < page.height / 2.0,
            "a destination is measured from the top of its page, like everything else: \
             {top} of {}",
            page.height
        );

        for link in &found {
            assert!(
                link.width > 0.0 && link.height > 0.0,
                "a link has a box to click: {link:?}"
            );
            assert!(
                link.x >= 0.0
                    && link.y >= 0.0
                    && link.x + link.width <= page.width + 1.0
                    && link.y + link.height <= page.height + 1.0,
                "and it sits on the page, measured from its top left: {link:?} of {page:?}"
            );
        }
    }

    #[test]
    fn renders_measures_and_searches_a_real_document() {
        let Some((_guard, pdf)) = fixture() else {
            eprintln!("skipping: latexmk is not installed");
            return;
        };
        let document = open(&pdf).unwrap();
        assert_eq!(page_count(&document).unwrap(), 2);

        let pages = geometry(&document).unwrap();
        assert_eq!(pages.len(), 2);
        // A4 or letter, but definitely portrait and a sane size in points.
        assert!(pages[0].width > 400.0 && pages[0].width < 700.0);
        assert!(pages[0].height > pages[0].width);

        // 1.3 zoom on a retina display.
        let rendered = render_page(&document, 0, 2.6, false).unwrap();
        assert_eq!(
            rendered.samples().len(),
            rendered.width as usize * rendered.height as usize * 4,
            "pages arrive as RGBA, ready for ImageData"
        );
        // Opaque: a page is a sheet of paper, not a transparency.
        assert!(
            rendered
                .samples()
                .chunks_exact(4)
                .all(|pixel| pixel[3] == 255)
        );
        assert!(rendered.width > 1000);
        // A page with text on it is not a blank sheet.
        assert!(
            rendered.samples().iter().any(|&byte| byte < 200),
            "the rasterised page has ink on it"
        );

        // The header travels in front of the samples, in the same allocation.
        let framed = render_page(&document, 0, 2.6, false)
            .unwrap()
            .into_framed([1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&framed[..8], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(
            framed.len() - 8,
            rendered.width as usize * rendered.height as usize * 4
        );

        let extracted = words(&document, 0).unwrap();
        assert!(extracted.iter().any(|word| word.text == "Introduction"));
        let introduction = extracted
            .iter()
            .find(|word| word.text == "Introduction")
            .unwrap();
        assert!(introduction.width > 0.0 && introduction.height > 0.0);
        assert!(introduction.x >= 0.0 && introduction.x < pages[0].width);

        // Search finds the word on page two and not on page one.
        assert!(search_page(&document, 0, "kestrel").unwrap().is_empty());
        let hits = search_page(&document, 1, "kestrel").unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].width > 0.0);

        assert!(search_page(&document, 0, "").unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_pool_serves_pages_from_several_threads() {
        let Some((_guard, pdf)) = fixture() else {
            eprintln!("skipping: latexmk is not installed");
            return;
        };
        let pool = RenderPool::new(2);

        let geometry = pool.geometry(pdf.clone()).await.unwrap();
        assert_eq!(geometry.len(), 2);

        // Both pages at once, which is the point of having more than one thread.
        let (first, second) = tokio::join!(
            pool.render(pdf.clone(), 0, 2.0, false),
            pool.render(pdf.clone(), 1, 2.0, false)
        );
        assert!(first.unwrap().width > 0);
        assert!(second.unwrap().width > 0);

        let extracted = pool.words(pdf.clone(), 0).await.unwrap();
        assert!(extracted.iter().any(|word| word.text == "Introduction"));

        let hits = pool.search(pdf.clone(), "kestrel".into()).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page, 1);

        // A second request for the same document reuses the cached handle.
        assert!(pool.render(pdf, 0, 1.0, false).await.is_ok());
    }

    #[tokio::test]
    async fn an_abandoned_request_is_skipped() {
        let Some((_guard, pdf)) = fixture() else {
            eprintln!("skipping: latexmk is not installed");
            return;
        };
        let pool = RenderPool::new(1);
        let (reply, receive) = oneshot::channel();
        // A caller whose own future was dropped must not cost a render.
        drop(receive);
        assert!(pool.queue.push(Job::Render {
            path: pdf.clone(),
            page: 0,
            scale: 4.0,
            invert: false,
            reply,
        }));

        // The worker stays available for real work.
        assert!(pool.render(pdf, 0, 1.0, false).await.is_ok());
    }

    /// The page under the reader's eyes is the newest request, and it must not
    /// wait behind every page they have already scrolled past.
    #[test]
    fn the_queue_serves_the_newest_request_first() {
        let queue = Queue::default();
        let mut replies = Vec::new();
        for page in 0..3 {
            let (reply, receive) = oneshot::channel();
            replies.push(receive);
            assert!(queue.push(Job::Render {
                path: PathBuf::from("/paper.pdf"),
                page,
                scale: 1.0,
                invert: false,
                reply,
            }));
        }

        let taken = std::iter::from_fn(|| queue.take())
            .take(3)
            .map(|job| match job {
                Job::Render { page, .. } => page,
                _ => unreachable!("only renders were queued"),
            })
            .collect::<Vec<_>>();
        assert_eq!(taken, [2, 1, 0], "newest first");
    }

    /// A scroll that never stops must not pile up work without limit. What is
    /// given up on is the oldest page, which is the one furthest from the
    /// window — and never a one-shot request that something is still waiting on.
    #[test]
    fn a_runaway_scroll_gives_up_on_the_pages_furthest_behind() {
        let queue = Queue::default();
        let (geometry_reply, geometry_receive) = oneshot::channel();
        assert!(queue.push(Job::Geometry {
            path: PathBuf::from("/paper.pdf"),
            reply: geometry_reply,
        }));

        let mut replies = Vec::new();
        for page in 0..(MOST_PENDING_RENDERS + 8) {
            let (reply, receive) = oneshot::channel();
            replies.push(receive);
            assert!(queue.push(Job::Render {
                path: PathBuf::from("/paper.pdf"),
                page,
                scale: 1.0,
                invert: false,
                reply,
            }));
        }

        let waiting = queue.pending.lock().unwrap();
        assert_eq!(
            waiting.jobs.iter().filter(|job| job.is_render()).count(),
            MOST_PENDING_RENDERS
        );
        assert!(
            waiting.jobs.iter().any(|job| !job.is_render()),
            "the geometry request is still there to be answered"
        );
        drop(waiting);

        // The eight oldest pages were let go; their callers hear about it
        // rather than waiting forever.
        for receive in replies.iter_mut().take(8) {
            assert!(receive.try_recv().is_err(), "the sender was dropped");
        }
        drop(geometry_receive);
    }

    /// A PDF Press only shows is rebuilt under its own name by whatever owns
    /// it, and noticing that is the whole reason it is watched. A handle held
    /// across the rewrite resolves the new bytes through the old
    /// cross-reference table, so the pages it draws are not merely stale — they
    /// are wrong, and nothing evicts the entry that produced them.
    #[tokio::test]
    async fn a_document_rewritten_under_its_own_name_is_read_again() {
        let Some((held, first)) = fixture() else {
            eprintln!("skipping: latexmk is not installed");
            return;
        };
        let Some(second) = pages_fixture(held.path(), 5) else {
            eprintln!("skipping: latexmk is not installed");
            return;
        };

        // One fixed path, the way a PDF Press is only showing has one.
        let live = held.path().join("live.pdf");
        std::fs::copy(&first, &live).unwrap();

        let pool = RenderPool::new(1);
        assert_eq!(pool.geometry(live.clone()).await.unwrap().len(), 2);

        std::fs::write(&live, std::fs::read(&second).unwrap()).unwrap();

        assert_eq!(
            pool.geometry(live.clone()).await.unwrap().len(),
            5,
            "the pool reads the file that is there now, not the one it opened"
        );
        // And the pixels are the new document's, not the old table's reading of
        // the new bytes — which is neither edition.
        let redrawn = pool.render(live.clone(), 0, 1.0, false).await.unwrap();
        let truth = render_page(&open(&live).unwrap(), 0, 1.0, false).unwrap();
        assert_eq!(redrawn.samples(), truth.samples());
    }

    /// A document of a given length, so a rewrite is visible as a page count.
    fn pages_fixture(directory: &Path, pages: usize) -> Option<std::path::PathBuf> {
        let latexmk = crate::toolchain::resolve_executable("latexmk")?;
        let source = directory.join(format!("of{pages}.tex"));
        let body = "\\newpage A page.\n".repeat(pages.saturating_sub(1));
        std::fs::write(
            &source,
            format!(
                "\\documentclass{{article}}\n\\begin{{document}}\n\
                 \\section{{Rewritten}}\nA different document entirely.\n{body}\
                 \\end{{document}}\n"
            ),
        )
        .ok()?;
        let status = std::process::Command::new(latexmk)
            .args(["-pdf", "-interaction=nonstopmode"])
            .arg(format!("-outdir={}", directory.display()))
            .arg(&source)
            .current_dir(directory)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()?;
        if !status.success() {
            return None;
        }
        let pdf = directory.join(format!("of{pages}.pdf"));
        pdf.is_file().then_some(pdf)
    }

    /// Not an assertion so much as a measurement, printed with `--nocapture`.
    /// Point `PRESS_BENCH_PDF` at a real document to size up a heavier one.
    #[test]
    fn reports_timings() {
        let external = std::env::var_os("PRESS_BENCH_PDF").map(std::path::PathBuf::from);
        let held;
        let pdf = match external {
            Some(path) if path.is_file() => path,
            _ => {
                let Some((guard, pdf)) = fixture() else {
                    eprintln!("skipping: latexmk is not installed");
                    return;
                };
                held = guard;
                let _ = &held;
                pdf
            }
        };

        let bytes = std::fs::metadata(&pdf).map(|data| data.len()).unwrap_or(0);
        let started = Instant::now();
        let document = open(&pdf).unwrap();
        let count = page_count(&document).unwrap();
        let open_time = started.elapsed();

        let started = Instant::now();
        let _ = geometry(&document).unwrap();
        let geometry_time = started.elapsed();

        // The first page of a document pays for font loading and cache warming.
        // Reporting only that conflates a one-time cost with the per-page cost
        // that actually decides how scrolling feels.
        let started = Instant::now();
        let rendered = render_page(&document, 0, 2.6, false).unwrap();
        let cold_time = started.elapsed();

        let sample = count.min(8);
        let mut warm = Vec::new();
        for index in 0..sample {
            let started = Instant::now();
            let _ = render_page(&document, index, 2.6, false).unwrap();
            warm.push(started.elapsed());
        }
        warm.sort();
        let median = warm[warm.len() / 2];
        let slowest = *warm.last().unwrap();

        let started = Instant::now();
        let _ = render_page(&document, 0, 1.0, false).unwrap();
        let unscaled = started.elapsed();

        let started = Instant::now();
        let extracted = words(&document, 0).unwrap();
        let words_time = started.elapsed();

        let started = Instant::now();
        let mut hits = 0;
        for index in 0..count {
            hits += search_page(&document, index, "the").unwrap().len();
        }
        let search_time = started.elapsed();

        println!(
            "\n{}\n  {bytes} bytes, {count} pages\n  \
             open+count      {open_time:?}\n  \
             all geometry    {geometry_time:?}\n  \
             page 1 cold     {cold_time:?} -> {}x{} ({} MiB rgba)\n  \
             warm median     {median:?}   (over {sample} pages, slowest {slowest:?})\n  \
             page 1 @1.0x    {unscaled:?}\n  \
             page 1 words    {words_time:?} -> {} words\n  \
             search all      {search_time:?} -> {hits} hits\n",
            pdf.display(),
            rendered.width,
            rendered.height,
            rendered.samples().len() / (1024 * 1024),
            extracted.len(),
        );
    }
}
