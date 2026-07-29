//! Driving a download progress bar without leaking `indicatif` to the binary.
//!
//! `bibi_documents` reports per-chunk progress through a closure; the binary
//! decides whether a bar is on screen at all. The intermediate type here owns
//! the bar machinery so neither side has to, and so a `bibi` call site can
//! compose `Progress::visible` only when the runtime policy permits one.

/// One chunk of progress reported by an arXiv download.
///
/// `(bytes_received_so_far, content_length_if_known)`. `None` for the second
/// half means the server did not send a `Content-Length` header — the transfer
/// was chunked and the bar shows position only.
pub type ProgressEvent = (usize, Option<u64>);

/// A shared cell a [`Progress::recorder`] writes into.
///
/// Wrapping the standard `Arc<Mutex<Vec<_>>` shape behind methods keeps the
/// complex type out of every test that needs to read out what a download
/// reported.
#[derive(Clone)]
pub struct ProgressSink {
    inner: std::sync::Arc<std::sync::Mutex<Vec<ProgressEvent>>>,
}

impl ProgressSink {
    /// A fresh, empty sink.
    pub fn new() -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// A snapshot of every event recorded so far.
    pub fn events(&self) -> Vec<ProgressEvent> {
        self.inner.lock().expect("recorder sink").clone()
    }
}

impl Default for ProgressSink {
    fn default() -> Self {
        Self::new()
    }
}

/// Reports download progress to a stderr sink.
///
/// Constructed at the call site: [`silent`](Self::silent) is a no-op, and
/// [`visible`](Self::visible) owns an `indicatif::ProgressBar` drawn on
/// stderr. Both flow into the same [`on_chunk`](Self::on_chunk) entry point so
/// the crate that drives the download does not branch on visibility.
pub struct Progress {
    bar: Option<indicatif::ProgressBar>,
    on_chunk: Box<dyn FnMut(usize, Option<u64>) + Send>,
}

impl Progress {
    /// A no-op progress reporter. Every chunk is silently dropped.
    ///
    /// Tests and offline callers use this; the visible constructor is
    /// reserved for paths that actually stream bytes to a terminal.
    pub fn silent() -> Self {
        Self {
            bar: None,
            on_chunk: Box::new(|_, _| {}),
        }
    }

    /// Build a visible stderr bar reporting total and bytes received.
    ///
    /// `label` is the prefix the bar shows, conventionally the arXiv id.
    /// The bar is drawn on stderr at the workspace's `unicode-width` policy
    /// via the enabled `indicatif.unicode-width` feature.
    pub fn visible(label: impl Into<String>) -> Self {
        let prefix = label.into();
        let style = indicatif::ProgressStyle::default_bar()
            .template("{prefix:.bold} [{bar:30.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .expect("the bar template is correct");
        let bar = indicatif::ProgressBar::new(0)
            .with_prefix(prefix)
            .with_style(style);
        bar.set_draw_target(indicatif::ProgressDrawTarget::stderr());
        let cloned = bar.clone();
        let on_chunk: Box<dyn FnMut(usize, Option<u64>) + Send> = Box::new(move |bytes, total| {
            if let Some(total) = total {
                cloned.set_length(total);
            }
            cloned.set_position(bytes as u64);
        });
        Self {
            bar: Some(bar),
            on_chunk,
        }
    }

    /// Build a progress reporter from a callable.
    ///
    /// Tests drive downloads through a recording closure without the terminal
    /// being involved. Production code does not use this constructor.
    pub fn from_fn<F>(on_chunk: F) -> Self
    where
        F: FnMut(usize, Option<u64>) + Send + 'static,
    {
        Self {
            bar: None,
            on_chunk: Box::new(on_chunk),
        }
    }

    /// Build a recording `Progress` and return its sink alongside it.
    ///
    /// Tests use this to assert what a download reported without involvement
    /// of the terminal: every [`on_chunk`](Self::on_chunk) call appends to the
    /// sink, which the caller can clone out at any time.
    pub fn recorder() -> (ProgressSink, Self) {
        let sink = ProgressSink::new();
        let receiver = sink.clone();
        let progress = Self::from_fn(move |bytes, total| {
            receiver
                .inner
                .lock()
                .expect("recorder sink")
                .push((bytes, total));
        });
        (sink, progress)
    }

    /// Invoke the configured reporter with one chunk's progress.
    ///
    /// Called once per response chunk by
    /// `bibi_documents::ArtifactClient::download_with_progress`.
    pub fn on_chunk(&mut self, bytes: usize, total: Option<u64>) {
        (self.on_chunk)(bytes, total);
    }

    /// Tear the bar down. Idempotent; safe to call repeatedly.
    pub fn finish(&self) {
        if let Some(bar) = &self.bar {
            bar.finish_and_clear();
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        // A panic in caller code must not leave a stranded bar on screen.
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_silent_progress_accepts_every_chunk_without_recording() {
        let mut progress = Progress::silent();
        progress.on_chunk(0, None);
        progress.on_chunk(4096, Some(8192));
        progress.on_chunk(8192, Some(8192));
        progress.finish();
    }

    #[test]
    fn a_recorder_progress_receives_every_event() {
        let (sink, mut progress) = Progress::recorder();
        progress.on_chunk(0, Some(1024));
        progress.on_chunk(512, Some(1024));
        progress.on_chunk(1024, Some(1024));
        progress.finish();
        assert_eq!(
            sink.events(),
            vec![(0, Some(1024)), (512, Some(1024)), (1024, Some(1024)),],
        );
    }

    #[test]
    fn finish_is_idempotent() {
        let progress = Progress::silent();
        progress.finish();
        progress.finish();
    }
}
