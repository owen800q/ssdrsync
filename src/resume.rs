use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const MANIFEST_NAME: &str = ".ssdrsync.manifest.json";

#[derive(Serialize, Deserialize, Debug)]
pub struct Manifest {
    pub version: u32,
    pub source: String,
    pub started_at: String,
    pub files: HashMap<String, FileEntry>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FileEntry {
    pub size: u64,
    pub blake3: String,
    pub bytes_written: u64,
    pub status: FileStatus,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum FileStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "in_progress")]
    InProgress,
    #[serde(rename = "completed")]
    Completed,
}

impl Manifest {
    pub fn new(source: &str) -> Self {
        Self {
            version: 1,
            source: source.to_string(),
            started_at: chrono_now(),
            files: HashMap::new(),
        }
    }

    pub fn manifest_path(dest_dir: &Path) -> PathBuf {
        dest_dir.join(MANIFEST_NAME)
    }

    pub fn load(dest_dir: &Path) -> Option<Self> {
        let path = Self::manifest_path(dest_dir);
        let data = fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }

    pub fn save(&self, dest_dir: &Path) -> std::io::Result<()> {
        let path = Self::manifest_path(dest_dir);
        let data = serde_json::to_string_pretty(self)?;
        fs::write(path, data)
    }

    pub fn is_completed(&self, rel_path: &str) -> bool {
        self.files
            .get(rel_path)
            .map(|e| e.status == FileStatus::Completed)
            .unwrap_or(false)
    }

    pub fn mark_in_progress(&mut self, rel_path: &str, size: u64) {
        self.files.insert(
            rel_path.to_string(),
            FileEntry {
                size,
                blake3: String::new(),
                bytes_written: 0,
                status: FileStatus::InProgress,
            },
        );
    }

    pub fn mark_completed(&mut self, rel_path: &str, blake3: &str) {
        if let Some(entry) = self.files.get_mut(rel_path) {
            entry.status = FileStatus::Completed;
            entry.blake3 = blake3.to_string();
            entry.bytes_written = entry.size;
        }
    }

    pub fn cleanup(dest_dir: &Path) {
        let path = Self::manifest_path(dest_dir);
        let _ = fs::remove_file(path);
    }
}

fn chrono_now() -> String {
    // Simple ISO 8601 without pulling in chrono
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s_since_epoch", duration.as_secs())
}
