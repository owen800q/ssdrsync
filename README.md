# ssdrsync

Super-fast file sync tool for Linux, optimized for copying large files (33GB+) to NAS shares.

## Why?

`cp` takes 3+ hours for a 33GB Loki chunk file. `ssdrsync` does it at full disk speed, and subsequent syncs take **seconds** by only copying changed blocks.

**How it works:**

- **`copy_file_range()`** - same zero-copy kernel syscall as modern `cp`, matching its speed
- **Binary search delta sync** - finds changed blocks in O(log N) reads, then only copies what changed
- **O_DIRECT I/O** - bypasses page cache for NAS writes (avoids 33GB of cache thrashing)
- **Parallel transfers** - copies multiple files simultaneously
- **Smart skip** - uses mtime+size to skip unchanged files instantly
- **Resume** - interrupted transfers can be resumed
- **CI-friendly** - `--log-progress` prints plain-text status lines for Jenkins/cron

## Benchmark (GitHub Actions, 10GB file)

```
Full copy (single 10GB file):
┌─────────────────────────────────┬──────────┬─────────────┐
│ Method                          │ Time     │ Speed       │
├─────────────────────────────────┼──────────┼─────────────┤
│ cp                              │   48.01s │  213.28 MB/s│
│ rsync                           │   48.17s │  212.58 MB/s│
│ ssdrsync (copy_file_range)      │   51.40s │  199.22 MB/s│
└─────────────────────────────────┴──────────┴─────────────┘

Delta sync (1GB modified at end of 10GB file):
┌─────────────────────────────────┬──────────┐
│ Scenario                        │ Time     │
├─────────────────────────────────┼──────────┤
│ ssdrsync: no changes (skip)     │       0s │
│ ssdrsync: 1GB modified (delta)  │    2.39s │  ← 20x faster
│ rsync:    1GB modified (delta)  │   49.23s │
└─────────────────────────────────┴──────────┘
```

> Full copy speed matches cp/rsync (same syscall). The real win is **delta sync** - 2.39s vs 49.23s for rsync when only 1GB changed.

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

## Jenkins / CI Usage

Use `--log-progress` for plain-text progress lines (no terminal escape codes):

```bash
ssdrsync /data/loki/chunks /mnt/nas/backup/ \
  --log-progress 10 \
  --no-direct-io \
  -v
```

Output in Jenkins console:

```
Sync: 5 files to copy (33.50 GB bytes), 12 skipped, 0 to delete
[ssdrsync] 15.2% | 5.09 GB/33.50 GB | 1/5 files | 198.3 MB/s | ETA: 145s
[ssdrsync] 30.1% | 10.08 GB/33.50 GB | 2/5 files | 201.1 MB/s | ETA: 118s
[ssdrsync] DONE: 5/5 files, 33.50 GB/33.50 GB | 200.1 MB/s | 171.4s total
```

Example Jenkinsfile:

```groovy
pipeline {
    agent any
    triggers { cron('0 */6 * * *') }
    stages {
        stage('Backup Loki Chunks') {
            steps {
                sh '''
                    /usr/local/bin/ssdrsync \
                        /data/loki/chunks \
                        /mnt/nas/backup/loki/chunks \
                        -j 4 \
                        --log-progress 10 \
                        --no-direct-io \
                        -v
                '''
            }
        }
    }
}
```

## Options

```
  -j, --jobs <N>              Parallel file transfers [default: 4]
  -b, --block-size <SIZE>     Block size for delta comparison [default: 1M]
      --no-delta              Disable delta sync, always full copy
      --no-direct-io          Disable O_DIRECT (for CIFS/SMB mounts)
      --resume                Resume interrupted transfer
      --dry-run               Show what would be copied
      --delete                Delete dest files not in source
      --checksum              Use checksums for change detection
  -v, --verbose               Increase verbosity
      --no-progress           Disable progress bar
      --log-progress [SECS]   Print progress as log lines [default: 5s]
```

## How Delta Sync Works

When a file exists at the destination, ssdrsync uses **binary search** to find where changes start:

1. Check first and last block of the file (2 reads)
2. Binary search for the exact change boundary (~14 reads for a 10GB file)
3. Copy only the changed blocks

For Loki chunk files that grow by appending data, this means only the new data gets copied. A 33GB file that grew by 500MB takes ~3 seconds instead of re-copying 33GB.

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
