use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "ssdrsync",
    about = "Super-fast file sync tool for large files on Linux",
    long_about = "A high-performance file synchronization tool optimized for copying large files (33GB+) \
                  to NAS shares. Uses O_DIRECT I/O, delta sync, and parallel transfers for maximum speed."
)]
pub struct Cli {
    /// Source file or directory
    pub source: String,

    /// Destination file or directory
    pub dest: String,

    /// Number of parallel file transfers
    #[arg(short = 'j', long, default_value_t = 4)]
    pub jobs: usize,

    /// Block size for I/O and delta sync (e.g. 1M, 4M, 512K)
    #[arg(short = 'b', long, default_value = "1M")]
    pub block_size: String,

    /// Disable delta sync, always do full copy
    #[arg(long)]
    pub no_delta: bool,

    /// Disable O_DIRECT (needed for some NAS/CIFS mounts)
    #[arg(long)]
    pub no_direct_io: bool,

    /// Resume interrupted transfer
    #[arg(long)]
    pub resume: bool,

    /// Show what would be copied without actually copying
    #[arg(long)]
    pub dry_run: bool,

    /// Delete files in destination not present in source
    #[arg(long)]
    pub delete: bool,

    /// Use checksums instead of mtime+size for change detection
    #[arg(long)]
    pub checksum: bool,

    /// Verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Disable progress bar
    #[arg(long)]
    pub no_progress: bool,
}

impl Cli {
    pub fn block_size_bytes(&self) -> usize {
        parse_size(&self.block_size).unwrap_or(1024 * 1024)
    }
}

fn parse_size(s: &str) -> Option<usize> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let (num_str, multiplier) = if s.ends_with('K') || s.ends_with('k') {
        (&s[..s.len() - 1], 1024usize)
    } else if s.ends_with('M') || s.ends_with('m') {
        (&s[..s.len() - 1], 1024 * 1024)
    } else if s.ends_with('G') || s.ends_with('g') {
        (&s[..s.len() - 1], 1024 * 1024 * 1024)
    } else {
        (s, 1usize)
    };

    num_str.parse::<usize>().ok().map(|n| n * multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("1M"), Some(1024 * 1024));
        assert_eq!(parse_size("4M"), Some(4 * 1024 * 1024));
        assert_eq!(parse_size("512K"), Some(512 * 1024));
        assert_eq!(parse_size("1G"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_size("4096"), Some(4096));
    }
}
