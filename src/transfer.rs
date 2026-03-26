use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::checksum;
use crate::error::Result;
use crate::io_engine;

pub struct TransferOptions {
    pub block_size: usize,
    pub use_direct_io: bool,
    pub use_delta: bool,
}

pub struct TransferResult {
    pub bytes_transferred: u64,
    pub skipped: bool,
    pub delta_blocks: Option<usize>,
}

/// Copy a single file from src to dst using the best available method
pub fn copy_file(
    src: &Path,
    dst: &Path,
    opts: &TransferOptions,
    progress_cb: impl FnMut(u64),
) -> Result<TransferResult> {
    let src_meta = fs::metadata(src)?;
    let src_size = src_meta.len();

    // Ensure parent directory exists
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    // Try delta sync first if destination exists and delta is enabled
    if opts.use_delta && dst.exists() {
        let dst_meta = fs::metadata(dst)?;
        let dst_size = dst_meta.len();

        return delta_copy(src, dst, src_size, dst_size, opts, progress_cb);
    }

    // Full copy
    full_copy(src, dst, src_size, opts, progress_cb)
}

fn delta_copy(
    src: &Path,
    dst: &Path,
    src_size: u64,
    dst_size: u64,
    opts: &TransferOptions,
    mut progress_cb: impl FnMut(u64),
) -> Result<TransferResult> {
    let mut src_file = File::open(src)?;
    let mut dst_file = OpenOptions::new().read(true).write(true).open(dst)?;

    let diff = checksum::diff_blocks(
        &mut src_file,
        &mut dst_file,
        src_size,
        dst_size,
        opts.block_size,
    )?;

    let total_blocks = diff.total_blocks_to_transfer();
    if total_blocks == 0 && !diff.truncate {
        return Ok(TransferResult {
            bytes_transferred: 0,
            skipped: true,
            delta_blocks: Some(0),
        });
    }

    let mut bytes_transferred: u64 = 0;
    let mut buf = vec![0u8; opts.block_size];

    // Copy changed and new blocks
    for &block_idx in diff.all_blocks() {
        let offset = block_idx as u64 * opts.block_size as u64;
        let remaining = src_size.saturating_sub(offset);
        let read_size = remaining.min(opts.block_size as u64) as usize;

        src_file.seek(SeekFrom::Start(offset))?;
        let n = read_full(&mut src_file, &mut buf[..read_size])?;
        if n == 0 {
            break;
        }

        dst_file.seek(SeekFrom::Start(offset))?;
        dst_file.write_all(&buf[..n])?;

        bytes_transferred += n as u64;
        progress_cb(n as u64);
    }

    // Truncate if source is smaller
    if diff.truncate {
        dst_file.set_len(diff.new_size)?;
    }

    dst_file.flush()?;

    Ok(TransferResult {
        bytes_transferred,
        skipped: false,
        delta_blocks: Some(total_blocks),
    })
}

fn full_copy(
    src: &Path,
    dst: &Path,
    src_size: u64,
    opts: &TransferOptions,
    mut progress_cb: impl FnMut(u64),
) -> Result<TransferResult> {
    // Try O_DIRECT first
    if opts.use_direct_io && src_size > 0 {
        if let Ok(result) = try_direct_copy(src, dst, src_size, opts, &mut progress_cb) {
            return Ok(result);
        }
    }

    // Try splice
    if src_size > 0 {
        if let Ok(result) = try_splice_copy(src, dst, src_size, opts, &mut progress_cb) {
            return Ok(result);
        }
    }

    // Fallback to buffered copy
    buffered_copy(src, dst, src_size, opts, progress_cb)
}

fn try_direct_copy(
    src: &Path,
    dst: &Path,
    src_size: u64,
    opts: &TransferOptions,
    progress_cb: &mut impl FnMut(u64),
) -> std::result::Result<TransferResult, ()> {
    let mut src_file = io_engine::open_direct_read(src).map_err(|_| ())?;
    let mut dst_file = io_engine::open_direct_write(dst).map_err(|_| ())?;

    match io_engine::direct_copy(&mut src_file, &mut dst_file, src_size, opts.block_size, progress_cb) {
        Ok(bytes) => Ok(TransferResult {
            bytes_transferred: bytes,
            skipped: false,
            delta_blocks: None,
        }),
        Err(_) => Err(()),
    }
}

fn try_splice_copy(
    src: &Path,
    dst: &Path,
    src_size: u64,
    opts: &TransferOptions,
    progress_cb: &mut impl FnMut(u64),
) -> std::result::Result<TransferResult, ()> {
    let src_file = File::open(src).map_err(|_| ())?;
    let dst_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dst)
        .map_err(|_| ())?;

    io_engine::preallocate(&dst_file, src_size);

    match io_engine::try_splice_copy(&src_file, &dst_file, src_size, opts.block_size, progress_cb) {
        Ok(bytes) => Ok(TransferResult {
            bytes_transferred: bytes,
            skipped: false,
            delta_blocks: None,
        }),
        Err(_) => Err(()),
    }
}

fn buffered_copy(
    src: &Path,
    dst: &Path,
    src_size: u64,
    opts: &TransferOptions,
    mut progress_cb: impl FnMut(u64),
) -> Result<TransferResult> {
    let mut src_file = File::open(src)?;
    let mut dst_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dst)?;

    io_engine::preallocate(&dst_file, src_size);

    let bytes = io_engine::buffered_copy(
        &mut src_file,
        &mut dst_file,
        src_size,
        opts.block_size,
        &mut progress_cb,
    )?;

    Ok(TransferResult {
        bytes_transferred: bytes,
        skipped: false,
        delta_blocks: None,
    })
}

fn read_full(file: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match file.read(&mut buf[total..])? {
            0 => break,
            n => total += n,
        }
    }
    Ok(total)
}
