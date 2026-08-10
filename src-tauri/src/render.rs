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
pub struct RenderedPage {
    pub width: u32,
    pub height: u32,
    pub samples: Vec<u8>,
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
    let mut samples = to_rgba(pixmap.samples(), pixmap.n());
    if invert {
        darken(&mut samples);
    }
    Ok(RenderedPage {
        width: pixmap.width(),
        height: pixmap.height(),
        samples,
    })
}

fn to_rgba(samples: &[u8], components: u8) -> Vec<u8> {
    match components {
        4 => samples.to_vec(),
        3 => {
            let mut rgba = Vec::with_capacity(samples.len() / 3 * 4);
            for pixel in samples.chunks_exact(3) {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
            rgba
        }
        1 => {
            let mut rgba = Vec::with_capacity(samples.len() * 4);
            for &grey in samples {
                rgba.extend_from_slice(&[grey, grey, grey, 255]);
            }
            rgba
        }
        _ => Vec::new(),
    }
}

/// Turns a page inside out for reading in the dark: paper goes black, ink goes
/// white, and everything with a colour keeps it.
///
/// Only the brightness is flipped. Each pixel's luminance is inverted the way a
/// grey would be — through sRGB's own curve, so mid grey lands back on mid grey
/// — and the three channels are then scaled together to reach it. Scaling them
/// together is what keeps the colour: the ratios between them are untouched, so
/// a red curve comes back red rather than cyan, which is where a per-channel
/// inversion sends it. MuPDF's own `invert` and `tint` are both per-channel.
///
/// Two honest costs. A light saturated colour has to come back dark to have
/// been inverted at all, so yellow reads as olive. And a colour whose scaled
/// form leaves what a screen can show is clipped to the edge of it, which is
/// why this does not perfectly undo itself for anything vivid.
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
        let target = LINEAR[flipped as usize];

        if luminance <= f32::EPSILON {
            // Black has no colour to keep, and nothing to scale by.
            pixel[0] = flipped;
            pixel[1] = flipped;
            pixel[2] = flipped;
            continue;
        }
        let factor = target / luminance;
        pixel[0] = encoded(red * factor);
        pixel[1] = encoded(green * factor);
        pixel[2] = encoded(blue * factor);
    }
}

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
    sync::{Arc, Mutex, mpsc},
};

use tokio::sync::oneshot;

/// Documents kept open per worker. Reopening costs about 4ms, so a handful is
/// plenty to keep scrolling and a side-by-side comparison warm.
const DOCUMENTS_PER_WORKER: usize = 4;

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
    /// True when whoever asked has gone away, so the work can be skipped
    /// entirely. Scrolling past a page before it is drawn costs nothing.
    fn abandoned(&self) -> bool {
        match self {
            Self::Render { reply, .. } => reply.is_closed(),
            Self::Geometry { reply, .. } => reply.is_closed(),
            Self::Words { reply, .. } => reply.is_closed(),
            Self::Links { reply, .. } => reply.is_closed(),
            Self::Search { reply, .. } => reply.is_closed(),
        }
    }
}

/// A few threads, each owning its own MuPDF context and open documents.
pub struct RenderPool {
    sender: mpsc::Sender<Job>,
}

impl RenderPool {
    pub fn new(workers: usize) -> Self {
        let (sender, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));
        for index in 0..workers.max(1) {
            let receiver = Arc::clone(&receiver);
            std::thread::Builder::new()
                .name(format!("press-render-{index}"))
                .spawn(move || worker(receiver))
                .expect("could not start a render thread");
        }
        Self { sender }
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
        self.sender
            .send(make(reply))
            .map_err(|_| AppError::Task("the render pool has stopped".into()))?;
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

/// Open documents for one thread. MuPDF's context belongs to the thread that
/// made it, so this is deliberately not shared.
struct DocumentCache {
    entries: VecDeque<(PathBuf, Document)>,
}

impl DocumentCache {
    fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    fn get(&mut self, path: &Path) -> AppResult<&Document> {
        if let Some(position) = self
            .entries
            .iter()
            .position(|(cached, _)| cached.as_path() == path)
        {
            let entry = self.entries.remove(position).expect("position is in range");
            self.entries.push_back(entry);
        } else {
            let document = open(path)?;
            self.entries.push_back((path.to_path_buf(), document));
            while self.entries.len() > DOCUMENTS_PER_WORKER {
                self.entries.pop_front();
            }
        }
        Ok(&self.entries.back().expect("just inserted").1)
    }
}

fn worker(receiver: Arc<Mutex<mpsc::Receiver<Job>>>) {
    let mut cache = DocumentCache::new();
    loop {
        // The lock is held only to take a job, never across the work itself.
        let job = {
            let Ok(guard) = receiver.lock() else {
                return;
            };
            match guard.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        };
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
                let result = cache
                    .get(&path)
                    .and_then(|document| words(document, page));
                let _ = reply.send(result);
            }
            Job::Links { path, page, reply } => {
                let result = cache
                    .get(&path)
                    .and_then(|document| links(document, page));
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
        // Paper and ink trade places.
        assert_eq!(inverted(255, 255, 255), [0, 0, 0]);
        assert_eq!(inverted(0, 0, 0), [255, 255, 255]);

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

        // Mid grey is close to where it started, which is what makes the flip
        // symmetric rather than a darkening.
        let grey = inverted(128, 128, 128);
        assert!(
            grey.iter().all(|channel| (100..=160).contains(channel)),
            "mid grey stays mid: {grey:?}"
        );
    }

    /// Twice over is where it started, for everything a screen can hold at both
    /// ends of the flip. A vivid colour is not one of those: inverting it takes
    /// it past what can be shown, it is clipped there, and what was clipped
    /// away cannot come back. Close enough to still be the same colour is all
    /// that is promised.
    #[test]
    fn inverting_twice_returns_the_page() {
        for grey in [0, 64, 128, 200, 255] {
            let once = inverted(grey, grey, grey);
            let twice = inverted(once[0], once[1], once[2]);
            assert_eq!(twice, [grey, grey, grey], "a grey is exactly reversible");
        }

        for original in [[140, 90, 90], [90, 110, 140], [30, 30, 30]] {
            let once = inverted(original[0], original[1], original[2]);
            let twice = inverted(once[0], once[1], once[2]);
            for (before, after) in original.iter().zip(twice.iter()) {
                assert!(
                    before.abs_diff(*after) <= 3,
                    "{original:?} came back as {twice:?}"
                );
            }
        }
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
        assert!(inward.uri.is_none(), "a destination inside is not an address");
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
            rendered.samples.len(),
            rendered.width as usize * rendered.height as usize * 4,
            "pages arrive as RGBA, ready for ImageData"
        );
        // Opaque: a page is a sheet of paper, not a transparency.
        assert!(rendered.samples.chunks_exact(4).all(|pixel| pixel[3] == 255));
        assert!(rendered.width > 1000);
        // A page with text on it is not a blank sheet.
        assert!(
            rendered.samples.iter().any(|&byte| byte < 200),
            "the rasterised page has ink on it"
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
        // Scrolling past a page before it is drawn must not cost a render.
        drop(receive);
        pool.sender
            .send(Job::Render {
                path: pdf.clone(),
                page: 0,
                scale: 4.0,
                invert: false,
                reply,
            })
            .unwrap();

        // The worker stays available for real work.
        assert!(pool.render(pdf, 0, 1.0, false).await.is_ok());
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
            rendered.samples.len() / (1024 * 1024),
            extracted.len(),
        );
    }
}

