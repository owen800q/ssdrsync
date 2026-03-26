use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::sync::Semaphore;
use walkdir::WalkDir;

use crate::cli::Cli;
use crate::error::{Result, SsdrError};
use crate::metadata;
use crate::progress::SyncProgress;
use crate::resume::Manifest;
use crate::transfer::{self, TransferOptions};

#[derive(Debug)]
enum SyncAction {
    Copy {
        src: PathBuf,
        dst: PathBuf,
        size: u64,
        rel_path: String,
    },
    Skip {
        rel_path: String,
    },
    Delete {
        path: PathBuf,
        rel_path: String,
    },
    CreateDir {
        path: PathBuf,
    },
}

pub async fn run_sync(cli: &Cli) -> Result<()> {
    let src = Path::new(&cli.source).canonicalize().map_err(|_| SsdrError::SourceNotFound {
        path: cli.source.clone(),
    })?;

    let dst = Path::new(&cli.dest);

    // Single file mode
    if src.is_file() {
        return sync_single_file(&src, dst, cli).await;
    }

    // Directory mode
    if !src.is_dir() {
        return Err(SsdrError::SourceNotFound {
            path: cli.source.clone(),
        });
    }

    fs::create_dir_all(dst)?;
    let dst = dst.canonicalize()?;

    // Plan sync actions
    let actions = plan_sync(&src, &dst, cli)?;

    // Calculate totals for progress
    let total_bytes: u64 = actions
        .iter()
        .filter_map(|a| match a {
            SyncAction::Copy { size, .. } => Some(*size),
            _ => None,
        })
        .sum();
    let total_files: u64 = actions
        .iter()
        .filter(|a| matches!(a, SyncAction::Copy { .. }))
        .count() as u64;
    let skip_count = actions
        .iter()
        .filter(|a| matches!(a, SyncAction::Skip { .. }))
        .count();
    let delete_count = actions
        .iter()
        .filter(|a| matches!(a, SyncAction::Delete { .. }))
        .count();

    let progress = Arc::new(SyncProgress::new(total_files, total_bytes, !cli.no_progress));

    progress.println(&format!(
        "Sync: {} files to copy ({} bytes), {} skipped, {} to delete",
        total_files,
        format_bytes(total_bytes),
        skip_count,
        delete_count,
    ));

    if cli.dry_run {
        for action in &actions {
            match action {
                SyncAction::Copy { rel_path, size, .. } => {
                    progress.println(&format!("  COPY {} ({})", rel_path, format_bytes(*size)));
                }
                SyncAction::Skip { rel_path } => {
                    if cli.verbose > 0 {
                        progress.println(&format!("  SKIP {}", rel_path));
                    }
                }
                SyncAction::Delete { rel_path, .. } => {
                    progress.println(&format!("  DELETE {}", rel_path));
                }
                SyncAction::CreateDir { path } => {
                    progress.println(&format!("  MKDIR {}", path.display()));
                }
            }
        }
        return Ok(());
    }

    // Load or create manifest for resume
    let manifest = if cli.resume {
        Manifest::load(&dst).unwrap_or_else(|| Manifest::new(&cli.source))
    } else {
        Manifest::new(&cli.source)
    };
    let manifest = Arc::new(Mutex::new(manifest));

    // Execute sync actions
    let semaphore = Arc::new(Semaphore::new(cli.jobs));
    let block_size = cli.block_size_bytes();
    let use_direct_io = !cli.no_direct_io;
    let use_delta = !cli.no_delta;
    let verbose = cli.verbose;

    // First: create directories
    for action in &actions {
        if let SyncAction::CreateDir { path } = action {
            fs::create_dir_all(path)?;
        }
    }

    // Then: delete files if requested
    for action in &actions {
        if let SyncAction::Delete { path, rel_path } = action {
            if path.is_dir() {
                fs::remove_dir_all(path)?;
            } else {
                fs::remove_file(path)?;
            }
            progress.println(&format!("  Deleted: {}", rel_path));
        }
    }

    // Finally: copy files in parallel
    let mut handles = Vec::new();
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));

    for action in actions {
        if let SyncAction::Copy {
            src,
            dst,
            size,
            rel_path,
        } = action
        {
            // Check resume manifest
            {
                let m = manifest.lock().unwrap();
                if m.is_completed(&rel_path) {
                    progress.inc_overall(size);
                    continue;
                }
            }

            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let progress = progress.clone();
            let manifest = manifest.clone();
            let errors = errors.clone();
            let handle = tokio::task::spawn_blocking(move || {
                let file_bar = progress.create_file_bar(&rel_path, size);

                // Mark in progress
                {
                    let mut m = manifest.lock().unwrap();
                    m.mark_in_progress(&rel_path, size);
                }

                let opts = TransferOptions {
                    block_size,
                    use_direct_io,
                    use_delta,
                };

                let result = transfer::copy_file(&src, &dst, &opts, |bytes| {
                    file_bar.inc(bytes);
                    progress.inc_overall(bytes);
                });

                match result {
                    Ok(tr) => {
                        // Copy metadata
                        if let Err(e) = metadata::copy_metadata(&src, &dst) {
                            if verbose > 0 {
                                progress.println(&format!(
                                    "  Warning: failed to copy metadata for {}: {}",
                                    rel_path, e
                                ));
                            }
                        }

                        // Mark completed
                        {
                            let mut m = manifest.lock().unwrap();
                            m.mark_completed(&rel_path, "");
                        }

                        if tr.skipped {
                            if verbose > 0 {
                                progress.println(&format!("  Delta skip: {} (identical)", rel_path));
                            }
                            progress.inc_overall(size);
                        } else if let Some(blocks) = tr.delta_blocks {
                            if verbose > 0 {
                                progress.println(&format!(
                                    "  Delta: {} ({} blocks, {} transferred)",
                                    rel_path,
                                    blocks,
                                    format_bytes(tr.bytes_transferred)
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        errors.lock().unwrap().push(format!("{}: {}", rel_path, e));
                        progress.println(&format!("  ERROR: {}: {}", rel_path, e));
                    }
                }

                progress.file_done(&file_bar);
                drop(permit);
            });
            handles.push(handle);
        }
    }

    // Wait for all transfers
    for handle in handles {
        let _ = handle.await;
    }

    // Save manifest
    {
        let m = manifest.lock().unwrap();
        let _ = m.save(&Path::new(&cli.dest).canonicalize().unwrap_or_else(|_| PathBuf::from(&cli.dest)));
    }

    progress.finish();

    let errs = errors.lock().unwrap();
    if !errs.is_empty() {
        eprintln!("\n{} errors occurred:", errs.len());
        for e in errs.iter() {
            eprintln!("  {}", e);
        }
    }

    Ok(())
}

async fn sync_single_file(src: &Path, dst: &Path, cli: &Cli) -> Result<()> {
    let src_meta = fs::metadata(src)?;
    let src_size = src_meta.len();

    let actual_dst = if dst.is_dir() {
        dst.join(src.file_name().unwrap())
    } else {
        dst.to_path_buf()
    };

    // Check if skip
    if !cli.checksum {
        if actual_dst.exists() {
            if let Ok(true) = metadata::same_mtime_size(src, &actual_dst) {
                println!("Skip: {} (same mtime+size)", src.display());
                return Ok(());
            }
        }
    }

    let progress = SyncProgress::new(1, src_size, !cli.no_progress);
    let file_bar = progress.create_file_bar(
        src.file_name().unwrap().to_str().unwrap_or("file"),
        src_size,
    );

    let opts = TransferOptions {
        block_size: cli.block_size_bytes(),
        use_direct_io: !cli.no_direct_io,
        use_delta: !cli.no_delta,
    };

    if cli.dry_run {
        println!("Would copy: {} -> {} ({})", src.display(), actual_dst.display(), format_bytes(src_size));
        return Ok(());
    }

    println!(
        "Copying: {} -> {} ({})",
        src.display(),
        actual_dst.display(),
        format_bytes(src_size)
    );

    let src_owned = src.to_owned();
    let dst_owned = actual_dst.clone();
    let result = tokio::task::spawn_blocking(move || {
        transfer::copy_file(&src_owned, &dst_owned, &opts, |bytes| {
            file_bar.inc(bytes);
        })
    })
    .await
    .map_err(|e| SsdrError::Other(e.to_string()))??;

    metadata::copy_metadata(src, &actual_dst)?;

    progress.finish();

    if let Some(blocks) = result.delta_blocks {
        println!(
            "Done: {} blocks changed, {} transferred",
            blocks,
            format_bytes(result.bytes_transferred)
        );
    } else {
        println!("Done: {} transferred", format_bytes(result.bytes_transferred));
    }

    Ok(())
}

fn plan_sync(src: &Path, dst: &Path, cli: &Cli) -> Result<Vec<SyncAction>> {
    let mut actions = Vec::new();

    // Walk source
    let mut src_entries: Vec<(String, PathBuf, bool, u64)> = Vec::new();
    for entry in WalkDir::new(src).follow_links(false) {
        let entry = entry.map_err(|e| SsdrError::Other(e.to_string()))?;
        let rel = entry
            .path()
            .strip_prefix(src)
            .unwrap()
            .to_string_lossy()
            .to_string();
        if rel.is_empty() {
            continue;
        }
        // Skip manifest files
        if rel.ends_with(".ssdrsync.manifest.json") {
            continue;
        }
        let is_dir = entry.file_type().is_dir();
        let size = if is_dir { 0 } else { entry.metadata().map(|m| m.len()).unwrap_or(0) };
        src_entries.push((rel, entry.path().to_path_buf(), is_dir, size));
    }

    // Build set of source relative paths
    let src_rel_set: std::collections::HashSet<&str> =
        src_entries.iter().map(|(rel, _, _, _)| rel.as_str()).collect();

    // Walk destination for delete detection
    if cli.delete {
        for entry in WalkDir::new(dst).follow_links(false) {
            let entry = entry.map_err(|e| SsdrError::Other(e.to_string()))?;
            let rel = entry
                .path()
                .strip_prefix(dst)
                .unwrap()
                .to_string_lossy()
                .to_string();
            if rel.is_empty() || rel.ends_with(".ssdrsync.manifest.json") {
                continue;
            }
            if !src_rel_set.contains(rel.as_str()) {
                actions.push(SyncAction::Delete {
                    path: entry.path().to_path_buf(),
                    rel_path: rel,
                });
            }
        }
    }

    // Plan copies
    for (rel, src_path, is_dir, size) in src_entries {
        let dst_path = dst.join(&rel);

        if is_dir {
            if !dst_path.exists() {
                actions.push(SyncAction::CreateDir { path: dst_path });
            }
            continue;
        }

        // Check if file needs copying
        if dst_path.exists() && !cli.checksum {
            if let Ok(true) = metadata::same_mtime_size(&src_path, &dst_path) {
                actions.push(SyncAction::Skip { rel_path: rel });
                continue;
            }
        }

        actions.push(SyncAction::Copy {
            src: src_path,
            dst: dst_path,
            size,
            rel_path: rel,
        });
    }

    Ok(actions)
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    const TB: u64 = 1024 * GB;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
