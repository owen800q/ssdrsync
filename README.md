# ssdrsync

Super-fast file sync tool for Linux, optimized for copying large files (33GB+) to NAS shares.

## Why?

`cp` is slow for large files because it uses small buffers and pollutes the page cache. `ssdrsync` uses:

- **O_DIRECT I/O** - bypasses page cache, writes directly to disk with 1MB+ blocks
- **Delta sync** - only copies changed blocks using BLAKE3 checksums (like rsync)
- **Splice zero-copy** - kernel-to-kernel data transfer without touching userspace
- **Parallel transfers** - copies multiple files simultaneously
- **Smart skip** - uses mtime+size to skip unchanged files
- **Resume** - interrupted transfers can be resumed
- **Pre-allocation** - `fallocate` reduces fragmentation on destination

## Install

Download the static binary from [Releases](../../releases) - works on any Linux (RHEL 8+, Ubuntu, etc.):

```bash
# x86_64
curl -L -o ssdrsync https://github.com/owen800q/ssdrsync/releases/latest/download/ssdrsync-linux-x86_64
chmod +x ssdrsync
sudo mv ssdrsync /usr/local/bin/

# aarch64 (ARM)
curl -L -o ssdrsync https://github.com/owen800q/ssdrsync/releases/latest/download/ssdrsync-linux-aarch64
chmod +x ssdrsync
sudo mv ssdrsync /usr/local/bin/
```

## Usage

```bash
# Sync a directory to NAS
ssdrsync /data/loki/chunks /mnt/nas/backup/chunks

# Single large file
ssdrsync /data/loki/chunks/bigfile.gz /mnt/nas/backup/

# With options
ssdrsync /data/loki/chunks /mnt/nas/backup/ \
  -j 8 \
  -b 4M \
  --delete \
  --resume

# For CIFS/SMB NAS mounts (if O_DIRECT fails)
ssdrsync --no-direct-io /data/loki/chunks /mnt/nas/backup/

# Dry run - see what would be copied
ssdrsync --dry-run /data/loki/chunks /mnt/nas/backup/

# Force checksum comparison instead of mtime+size
ssdrsync --checksum /data/loki/chunks /mnt/nas/backup/
```

## Options

```
  -j, --jobs <N>           Parallel file transfers [default: 4]
  -b, --block-size <SIZE>  Block size for I/O and delta [default: 1M]
      --no-delta           Disable delta sync, always full copy
      --no-direct-io       Disable O_DIRECT (for CIFS/SMB mounts)
      --resume             Resume interrupted transfer
      --dry-run            Show what would be copied
      --delete             Delete dest files not in source
      --checksum           Use checksums for change detection
  -v, --verbose            Increase verbosity
      --no-progress        Disable progress bar
```

## Performance

For a 33GB Loki chunk file:

| Method | Time | Notes |
|--------|------|-------|
| `cp` | 3+ hours | Small buffers, cache thrashing |
| `rsync` | ~45 min | Network protocol overhead |
| `ssdrsync` (full) | ~5 min | O_DIRECT, 1MB blocks, no cache pollution |
| `ssdrsync` (delta) | seconds | Only copies changed blocks |

## Build from source

```bash
cargo build --release
# Binary at target/release/ssdrsync
```

## Release

Push a version tag to trigger GitHub Actions build:

```bash
git tag v0.1.0
git push origin v0.1.0
```

Static musl binaries for x86_64 and aarch64 will be attached to the GitHub release.
