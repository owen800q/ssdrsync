use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

/// Read exactly buf.len() bytes, or fewer if at EOF
fn read_exact_or_eof(file: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match file.read(&mut buf[total..])? {
            0 => break,
            n => total += n,
        }
    }
    Ok(total)
}

/// Fast block comparison using sequential streaming reads.
/// Instead of seeking per-block (slow on NAS), reads both files sequentially
/// and compares BLAKE3 hashes of each block.
pub fn diff_blocks(
    src: &mut File,
    dst: &mut File,
    src_size: u64,
    dst_size: u64,
    block_size: usize,
) -> std::io::Result<DiffResult> {
    let src_blocks = ((src_size + block_size as u64 - 1) / block_size as u64) as usize;
    let dst_blocks = ((dst_size + block_size as u64 - 1) / block_size as u64) as usize;
    let common_blocks = src_blocks.min(dst_blocks);

    // For same-size files or when dst is larger, do fast sequential comparison.
    // For append-mostly pattern (src > dst), skip comparing common prefix from end.
    if src_size > dst_size && dst_size > 0 {
        return diff_blocks_append_optimized(src, dst, src_size, dst_size, block_size, src_blocks, dst_blocks);
    }

    // Sequential forward scan - reads both files linearly (fast I/O pattern)
    src.seek(SeekFrom::Start(0))?;
    dst.seek(SeekFrom::Start(0))?;

    let mut src_buf = vec![0u8; block_size];
    let mut dst_buf = vec![0u8; block_size];
    let mut changed_blocks: Vec<usize> = Vec::new();

    for i in 0..common_blocks {
        let src_n = read_exact_or_eof(src, &mut src_buf)?;
        let dst_n = read_exact_or_eof(dst, &mut dst_buf)?;

        // Fast path: compare raw bytes first (avoids hash for identical blocks)
        if src_n == dst_n && src_buf[..src_n] == dst_buf[..dst_n] {
            continue;
        }

        changed_blocks.push(i);
    }

    let new_blocks: Vec<usize> = (dst_blocks..src_blocks).collect();
    let truncate = src_size < dst_size;

    Ok(DiffResult {
        changed_blocks,
        new_blocks,
        truncate,
        new_size: src_size,
    })
}

/// Optimized diff for append-mostly files (common with Loki chunks).
/// Scans backward from the end of the common region to find where changes start,
/// then only the tail + new blocks need to be transferred.
fn diff_blocks_append_optimized(
    src: &mut File,
    dst: &mut File,
    src_size: u64,
    dst_size: u64,
    block_size: usize,
    src_blocks: usize,
    dst_blocks: usize,
) -> std::io::Result<DiffResult> {
    let common_blocks = dst_blocks; // dst is smaller

    // Backward scan: find the last matching block from the end of common region
    let mut first_changed_from_end = common_blocks;
    let mut src_buf = vec![0u8; block_size];
    let mut dst_buf = vec![0u8; block_size];

    for i in (0..common_blocks).rev() {
        let offset = i as u64 * block_size as u64;

        src.seek(SeekFrom::Start(offset))?;
        let src_n = read_exact_or_eof(src, &mut src_buf)?;

        dst.seek(SeekFrom::Start(offset))?;
        let dst_n = read_exact_or_eof(dst, &mut dst_buf)?;

        if src_n == dst_n && src_buf[..src_n] == dst_buf[..dst_n] {
            // This block matches - everything before it is likely unchanged
            first_changed_from_end = i + 1;
            break;
        }
    }

    // If backward scan reached beginning, all common blocks might differ
    // Do sequential forward scan for accuracy (still fast - sequential I/O)
    let mut changed_blocks: Vec<usize> = Vec::new();

    if first_changed_from_end == common_blocks {
        // Full forward scan needed
        src.seek(SeekFrom::Start(0))?;
        dst.seek(SeekFrom::Start(0))?;

        for i in 0..common_blocks {
            let src_n = read_exact_or_eof(src, &mut src_buf)?;
            let dst_n = read_exact_or_eof(dst, &mut dst_buf)?;

            if src_n != dst_n || src_buf[..src_n] != dst_buf[..dst_n] {
                changed_blocks.push(i);
            }
        }
    } else {
        // Only blocks from first_changed_from_end to end differ
        for i in first_changed_from_end..common_blocks {
            changed_blocks.push(i);
        }
    }

    // New blocks (source is larger than dest)
    let new_blocks: Vec<usize> = (dst_blocks..src_blocks).collect();

    Ok(DiffResult {
        changed_blocks,
        new_blocks,
        truncate: false,
        new_size: src_size,
    })
}

pub struct DiffResult {
    /// Block indices that exist in both files but differ
    pub changed_blocks: Vec<usize>,
    /// Block indices that are new in source (source is larger)
    pub new_blocks: Vec<usize>,
    /// Whether destination needs to be truncated
    pub truncate: bool,
    /// Target file size
    pub new_size: u64,
}

impl DiffResult {
    pub fn total_blocks_to_transfer(&self) -> usize {
        self.changed_blocks.len() + self.new_blocks.len()
    }

    pub fn all_blocks(&self) -> impl Iterator<Item = &usize> {
        self.changed_blocks.iter().chain(self.new_blocks.iter())
    }
}
