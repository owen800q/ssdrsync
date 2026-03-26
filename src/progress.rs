use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::sync::Arc;

pub struct SyncProgress {
    pub multi: Arc<MultiProgress>,
    pub overall: ProgressBar,
    enabled: bool,
}

impl SyncProgress {
    pub fn new(total_files: u64, total_bytes: u64, enabled: bool) -> Self {
        let multi = Arc::new(MultiProgress::new());

        let overall = if enabled {
            let pb = multi.add(ProgressBar::new(total_bytes));
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, ETA: {eta})")
                    .unwrap()
                    .progress_chars("#>-"),
            );
            pb.set_message(format!("{} files", total_files));
            pb
        } else {
            ProgressBar::hidden()
        };

        Self {
            multi,
            overall,
            enabled,
        }
    }

    pub fn create_file_bar(&self, filename: &str, size: u64) -> ProgressBar {
        if !self.enabled {
            return ProgressBar::hidden();
        }

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

    pub fn inc_overall(&self, bytes: u64) {
        self.overall.inc(bytes);
    }

    pub fn file_done(&self, pb: &ProgressBar) {
        pb.finish_and_clear();
    }

    pub fn finish(&self) {
        self.overall.finish_with_message("Done");
    }

    pub fn println(&self, msg: &str) {
        if self.enabled {
            let _ = self.multi.println(msg);
        } else {
            println!("{}", msg);
        }
    }
}
