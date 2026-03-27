use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub enum ProgressMode {
    /// Interactive terminal progress bars
    Bar,
    /// Plain text log lines at intervals (for Jenkins/CI)
    Log { interval_secs: u64 },
    /// No progress output
    Hidden,
}

pub struct SyncProgress {
    pub multi: Arc<MultiProgress>,
    pub overall: ProgressBar,
    mode: ProgressMode,
    total_bytes: u64,
    total_files: u64,
    bytes_done: Arc<AtomicU64>,
    files_done: Arc<AtomicU64>,
    start: Instant,
    last_log: Arc<std::sync::Mutex<Instant>>,
    log_interval_secs: u64,
}

impl SyncProgress {
    pub fn new(total_files: u64, total_bytes: u64, mode: ProgressMode) -> Self {
        let multi = Arc::new(MultiProgress::new());

        let (overall, log_interval_secs) = match &mode {
            ProgressMode::Bar => {
                let pb = multi.add(ProgressBar::new(total_bytes));
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, ETA: {eta})")
                        .unwrap()
                        .progress_chars("#>-"),
                );
                pb.set_message(format!("{} files", total_files));
                (pb, 0)
            }
            ProgressMode::Log { interval_secs } => {
                (ProgressBar::hidden(), *interval_secs)
            }
            ProgressMode::Hidden => {
                (ProgressBar::hidden(), 0)
            }
        };

        let now = Instant::now();

        Self {
            multi,
            overall,
            mode,
            total_bytes,
            total_files,
            bytes_done: Arc::new(AtomicU64::new(0)),
            files_done: Arc::new(AtomicU64::new(0)),
            start: now,
            last_log: Arc::new(std::sync::Mutex::new(now)),
            log_interval_secs,
        }
    }

    pub fn create_file_bar(&self, filename: &str, size: u64) -> ProgressBar {
        match &self.mode {
            ProgressMode::Bar => {
                let pb = self.multi.add(ProgressBar::new(size));
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template("  {msg:30!} [{bar:30.yellow/blue}] {bytes}/{total_bytes} {bytes_per_sec}")
                        .unwrap()
                        .progress_chars("=>-"),
                );
                let display_name = if filename.len() > 28 {
                    format!("...{}", &filename[filename.len() - 25..])
                } else {
                    filename.to_string()
                };
                pb.set_message(display_name);
                pb
            }
            _ => ProgressBar::hidden(),
        }
    }

    pub fn inc_overall(&self, bytes: u64) {
        self.overall.inc(bytes);
        self.bytes_done.fetch_add(bytes, Ordering::Relaxed);
        self.maybe_log_progress();
    }

    pub fn file_done(&self, pb: &ProgressBar) {
        pb.finish_and_clear();
        self.files_done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn finish(&self) {
        self.overall.finish_with_message("Done");

        if matches!(self.mode, ProgressMode::Log { .. }) {
            let elapsed = self.start.elapsed().as_secs_f64();
            let bytes = self.bytes_done.load(Ordering::Relaxed);
            let files = self.files_done.load(Ordering::Relaxed);
            let speed = if elapsed > 0.0 {
                bytes as f64 / 1048576.0 / elapsed
            } else {
                0.0
            };
            eprintln!(
                "[ssdrsync] DONE: {}/{} files, {}/{} | {:.1} MB/s | {:.1}s total",
                files,
                self.total_files,
                format_bytes(bytes),
                format_bytes(self.total_bytes),
                speed,
                elapsed,
            );
        }
    }

    pub fn println(&self, msg: &str) {
        match &self.mode {
            ProgressMode::Bar => {
                let _ = self.multi.println(msg);
            }
            _ => {
                eprintln!("{}", msg);
            }
        }
    }

    fn maybe_log_progress(&self) {
        if self.log_interval_secs == 0 {
            return;
        }

        let now = Instant::now();
        let mut last = self.last_log.lock().unwrap();
        if now.duration_since(*last).as_secs() < self.log_interval_secs {
            return;
        }
        *last = now;

        let elapsed = self.start.elapsed().as_secs_f64();
        let bytes = self.bytes_done.load(Ordering::Relaxed);
        let files = self.files_done.load(Ordering::Relaxed);
        let pct = if self.total_bytes > 0 {
            (bytes as f64 / self.total_bytes as f64 * 100.0).min(100.0)
        } else {
            0.0
        };
        let speed = if elapsed > 0.0 {
            bytes as f64 / 1048576.0 / elapsed
        } else {
            0.0
        };
        let eta = if speed > 0.0 && bytes < self.total_bytes {
            let remaining = (self.total_bytes - bytes) as f64 / 1048576.0 / speed;
            format!("{:.0}s", remaining)
        } else {
            "-".to_string()
        };

        eprintln!(
            "[ssdrsync] {:.1}% | {}/{} | {}/{} files | {:.1} MB/s | ETA: {}",
            pct,
            format_bytes(bytes),
            format_bytes(self.total_bytes),
            files,
            self.total_files,
            speed,
            eta,
        );
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
