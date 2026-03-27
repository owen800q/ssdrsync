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

/// Compare a single block at the given index
fn blocks_match(
    src: &mut File,
    dst: &mut File,
    block_idx: usize,
    block_size: usize,
    src_buf: &mut [u8],
    dst_buf: &mut [u8],
) -> std::io::Result<bool> {
    let offset = block_idx as u64 * block_size as u64;

    src.seek(SeekFrom::Start(offset))?;
    let src_n = read_exact_or_eof(src, src_buf)?;

    dst.seek(SeekFrom::Start(offset))?;
    let dst_n = read_exact_or_eof(dst, dst_buf)?;

    Ok(src_n == dst_n && src_buf[..src_n] == dst_buf[..dst_n])
}

/// Binary search for first DIFFERENT block in [lo, hi).
/// Blocks before the result are identical; block at result is different.
fn search_first_different(
    src: &mut File,
    dst: &mut File,
    lo: usize,
    hi: usize,
    block_size: usize,
    src_buf: &mut [u8],
    dst_buf: &mut [u8],
) -> std::io::Result<usize> {
    let mut lo = lo;
    let mut hi = hi;

    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if blocks_match(src, dst, mid, block_size, src_buf, dst_buf)? {
            lo = mid + 1; // mid is same, first diff is to the right
        } else {
            hi = mid; // mid is different, could be the first
        }
    }

    Ok(lo)
}

/// Binary search for first IDENTICAL block in [lo, hi).
/// Blocks before the result are different; block at result is identical.
fn search_first_identical(
    src: &mut File,
    dst: &mut File,
    lo: usize,
    hi: usize,
    block_size: usize,
    src_buf: &mut [u8],
    dst_buf: &mut [u8],
) -> std::io::Result<usize> {
    let mut lo = lo;
    let mut hi = hi;

    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if blocks_match(src, dst, mid, block_size, src_buf, dst_buf)? {
            hi = mid; // mid is same, could be the first same
        } else {
            lo = mid + 1; // mid is different, first same is to the right
        }
    }

    Ok(lo)
}

/// Compare blocks between source and destination files.
///
/// Strategy: Binary search for the change boundary.
/// For a 10GB file with 1GB changed at the end:
///   - ~14 block reads (log2 of 10240 blocks) to find boundary
///   - Then only transfer the 1024 changed blocks
///   - Total I/O: ~28MB scan + 1GB copy ≈ 5 seconds
///
/// Compare this to sequential scan: 20GB read ≈ 50 seconds.
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

    if common_blocks == 0 {
        return Ok(DiffResult {
            changed_blocks: Vec::new(),
            new_blocks: (dst_blocks..src_blocks).collect(),
            truncate: src_size < dst_size,
            new_size: src_size,
        });
    }

    let mut src_buf = vec![0u8; block_size];
    let mut dst_buf = vec![0u8; block_size];

    // Quick check: is the last common block identical?
    let last_same = blocks_match(
        src, dst, common_blocks - 1, block_size, &mut src_buf, &mut dst_buf,
    )?;

    if last_same {
        // Quick check: is the first block identical too?
        if common_blocks == 1 {
            // Single block file, already checked
            return Ok(DiffResult {
                changed_blocks: Vec::new(),
                new_blocks: (dst_blocks..src_blocks).collect(),
                truncate: src_size < dst_size,
                new_size: src_size,
            });
        }

        let first_same = blocks_match(
            src, dst, 0, block_size, &mut src_buf, &mut dst_buf,
        )?;

        if first_same {
            // Both ends match - likely identical or changes in the middle.
            // Do a fast sequential scan to find any middle changes.
            // Use sparse sampling first: check every Nth block.
            let step = (common_blocks / 32).max(1);
            let mut has_changes = false;

            for i in (0..common_blocks).step_by(step) {
                if !blocks_match(src, dst, i, block_size, &mut src_buf, &mut dst_buf)? {
                    has_changes = true;
                    break;
                }
            }

            if !has_changes {
                // Sampled blocks all match - very likely identical.
                // Do a full sequential verify.
                return full_sequential_diff(src, dst, common_blocks, src_blocks, dst_blocks, src_size, dst_size, block_size);
            }

            // Has middle changes - fall through to full sequential scan
            return full_sequential_diff(src, dst, common_blocks, src_blocks, dst_blocks, src_size, dst_size, block_size);
        }

        // First differs, last same - changes at the beginning.
        // Binary search for where blocks become identical again.
        let boundary = search_first_identical(
            src, dst, 0, common_blocks, block_size, &mut src_buf, &mut dst_buf,
        )?;
        // Blocks [0, boundary) are changed
        let changed_blocks: Vec<usize> = (0..boundary).collect();

        return Ok(DiffResult {
            changed_blocks,
            new_blocks: (dst_blocks..src_blocks).collect(),
            truncate: src_size < dst_size,
            new_size: src_size,
        });
    }

    // Last block differs. Check first block.
    let first_same = blocks_match(
        src, dst, 0, block_size, &mut src_buf, &mut dst_buf,
    )?;

    if first_same {
        // First same, last different → tail modification (most common case for Loki).
        // Binary search: find the first different block.
        let boundary = search_first_different(
            src, dst, 1, common_blocks, block_size, &mut src_buf, &mut dst_buf,
        )?;
        // Blocks [boundary, common_blocks) are changed
        let changed_blocks: Vec<usize> = (boundary..common_blocks).collect();

        return Ok(DiffResult {
            changed_blocks,
            new_blocks: (dst_blocks..src_blocks).collect(),
            truncate: src_size < dst_size,
            new_size: src_size,
        });
    }

    // Both first and last differ - widespread changes.
    // Fall through to full sequential scan.
    full_sequential_diff(src, dst, common_blocks, src_blocks, dst_blocks, src_size, dst_size, block_size)
}

/// Full sequential diff - reads both files linearly. Used as fallback
/// when changes aren't concentrated at head or tail.
fn full_sequential_diff(
    src: &mut File,
    dst: &mut File,
    common_blocks: usize,
    src_blocks: usize,
    dst_blocks: usize,
    src_size: u64,
    dst_size: u64,
    block_size: usize,
) -> std::io::Result<DiffResult> {
    src.seek(SeekFrom::Start(0))?;
    dst.seek(SeekFrom::Start(0))?;

    let mut src_buf = vec![0u8; block_size];
    let mut dst_buf = vec![0u8; block_size];
    let mut changed_blocks: Vec<usize> = Vec::new();

    for i in 0..common_blocks {
        let src_n = read_exact_or_eof(src, &mut src_buf)?;
        let dst_n = read_exact_or_eof(dst, &mut dst_buf)?;

        if src_n != dst_n || src_buf[..src_n] != dst_buf[..dst_n] {
            changed_blocks.push(i);
        }
    }

    Ok(DiffResult {
        changed_blocks,
        new_blocks: (dst_blocks..src_blocks).collect(),
        truncate: src_size < dst_size,
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
