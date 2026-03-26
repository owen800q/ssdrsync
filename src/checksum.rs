use std::fs::File;
use std::io::Read;

/// Compute BLAKE3 hash of a file block at a given offset
pub fn hash_block_at(file: &mut File, offset: u64, block_size: usize) -> std::io::Result<blake3::Hash> {
    use std::io::Seek;
    file.seek(std::io::SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; block_size];
    let n = read_exact_or_eof(file, &mut buf)?;
    Ok(blake3::hash(&buf[..n]))
}

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

/// Compare blocks between source and destination files.
/// Returns a list of block indices that differ (need to be transferred).
/// Scans from the end for append-mostly optimization.
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
    let mut changed_blocks: Vec<usize> = Vec::new();

    // Compare common blocks - scan from end (append-mostly optimization)
    // Once we find a matching block scanning backwards, all blocks before it
    // are likely unchanged too, so we can switch to forward scan for verification
    let mut backward_mismatch_end = common_blocks;

    // Backward scan: find where changes start from the end
    for i in (0..common_blocks).rev() {
        let offset = i as u64 * block_size as u64;
        let src_hash = hash_block_at(src, offset, block_size)?;
        let dst_hash = hash_block_at(dst, offset, block_size)?;
        if src_hash == dst_hash {
            backward_mismatch_end = i + 1;
            break;
        }
    }

    // If backward scan hit the beginning, check all blocks
    if backward_mismatch_end == common_blocks {
        // All blocks might differ, do forward scan
        for i in 0..common_blocks {
            let offset = i as u64 * block_size as u64;
            let src_hash = hash_block_at(src, offset, block_size)?;
            let dst_hash = hash_block_at(dst, offset, block_size)?;
            if src_hash != dst_hash {
                changed_blocks.push(i);
            }
        }
    } else {
        // Forward scan up to the first matching block from backward scan
        // and include all blocks from backward_mismatch_end to common_blocks
        for i in 0..backward_mismatch_end.saturating_sub(1) {
            let offset = i as u64 * block_size as u64;
            let src_hash = hash_block_at(src, offset, block_size)?;
            let dst_hash = hash_block_at(dst, offset, block_size)?;
            if src_hash != dst_hash {
                changed_blocks.push(i);
            }
        }
        // Add all blocks from the backward mismatch point to end
        for i in backward_mismatch_end..common_blocks {
            let offset = i as u64 * block_size as u64;
            let src_hash = hash_block_at(src, offset, block_size)?;
            let dst_hash = hash_block_at(dst, offset, block_size)?;
            if src_hash != dst_hash {
                changed_blocks.push(i);
            }
        }
    }

    // New blocks (source is larger)
    let new_blocks: Vec<usize> = (dst_blocks..src_blocks).collect();

    let truncate = src_size < dst_size;

    Ok(DiffResult {
        changed_blocks,
        new_blocks,
        truncate,
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
