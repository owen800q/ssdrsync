use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::path::Path;

const ALIGNMENT: usize = 4096;

/// Allocate a page-aligned buffer for O_DIRECT I/O
pub fn aligned_buffer(size: usize) -> AlignedBuffer {
    let aligned_size = (size + ALIGNMENT - 1) & !(ALIGNMENT - 1);
    let layout = std::alloc::Layout::from_size_align(aligned_size, ALIGNMENT).unwrap();
    let ptr = unsafe { std::alloc::alloc(layout) };
    if ptr.is_null() {
        std::alloc::handle_alloc_error(layout);
    }
    AlignedBuffer {
        ptr,
        size: aligned_size,
        layout,
    }
}

pub struct AlignedBuffer {
    ptr: *mut u8,
    size: usize,
    layout: std::alloc::Layout,
}

unsafe impl Send for AlignedBuffer {}

impl AlignedBuffer {
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.size) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.size) }
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        unsafe { std::alloc::dealloc(self.ptr, self.layout) }
    }
}

/// Try to open a file with O_DIRECT
pub fn open_direct_read(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECT)
        .open(path)
}

/// Try to open/create a file with O_DIRECT for writing
pub fn open_direct_write(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(libc::O_DIRECT)
        .open(path)
}

/// Set sequential read advice
pub fn advise_sequential(file: &File) {
    unsafe {
        libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_SEQUENTIAL);
    }
}

/// Tell kernel we don't need this data cached anymore
pub fn advise_dontneed(file: &File, offset: i64, len: i64) {
    unsafe {
        libc::posix_fadvise(file.as_raw_fd(), offset, len, libc::POSIX_FADV_DONTNEED);
    }
}

/// Pre-allocate space for destination file to reduce fragmentation
pub fn preallocate(file: &File, size: u64) {
    unsafe {
        libc::posix_fallocate(file.as_raw_fd(), 0, size as libc::off_t);
    }
}

/// copy_file_range - the fastest kernel copy path (what modern cp uses).
/// Zero-copy within kernel, supports server-side copy on NFS, reflink on btrfs/xfs.
pub fn copy_file_range_copy(
    src: &File,
    dst: &File,
    total_size: u64,
    block_size: usize,
    mut progress_cb: impl FnMut(u64),
) -> std::io::Result<u64> {
    let mut total: u64 = 0;
    let mut src_off: i64 = 0;
    let mut dst_off: i64 = 0;

    preallocate(dst, total_size);
    advise_sequential(src);

    while total < total_size {
        let remaining = total_size - total;
        let chunk = remaining.min(block_size as u64) as usize;

        let n = unsafe {
            libc::syscall(
                libc::SYS_copy_file_range,
                src.as_raw_fd(),
                &mut src_off as *mut i64,
                dst.as_raw_fd(),
                &mut dst_off as *mut i64,
                chunk,
                0u32, // flags
            )
        };

        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::ENOSYS)
                || err.raw_os_error() == Some(libc::EXDEV)
                || err.raw_os_error() == Some(libc::EINVAL)
            {
                // Not supported, caller should fallback
                return Err(err);
            }
            return Err(err);
        }
        if n == 0 {
            break; // EOF
        }

        let bytes = n as u64;
        total += bytes;
        progress_cb(bytes);

        // Drop page cache periodically to avoid memory pressure
        if total % (block_size as u64 * 32) == 0 {
            advise_dontneed(dst, 0, total as i64);
        }
    }

    Ok(total)
}

/// Attempt splice-based zero-copy transfer between two files.
/// Returns Ok(bytes_copied) or Err if splice is not supported.
pub fn try_splice_copy(
    src: &File,
    dst: &File,
    len: u64,
    block_size: usize,
    mut progress_cb: impl FnMut(u64),
) -> std::io::Result<u64> {
    let mut pipe_fds = [0i32; 2];
    let ret = unsafe { libc::pipe(pipe_fds.as_mut_ptr()) };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }

    // Set pipe size to block_size for better throughput
    unsafe {
        libc::fcntl(pipe_fds[1], libc::F_SETPIPE_SZ, block_size as libc::c_int);
    }

    let mut total: u64 = 0;
    let result = loop {
        let remaining = len - total;
        if remaining == 0 {
            break Ok(total);
        }
        let chunk = remaining.min(block_size as u64) as usize;

        // splice src -> pipe
        let n = unsafe {
            libc::splice(
                src.as_raw_fd(),
                std::ptr::null_mut(),
                pipe_fds[1],
                std::ptr::null_mut(),
                chunk,
                libc::SPLICE_F_MOVE | libc::SPLICE_F_MORE,
            )
        };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            break Err(err);
        }
        if n == 0 {
            break Ok(total); // EOF
        }

        // splice pipe -> dst
        let mut written: isize = 0;
        while written < n {
            let w = unsafe {
                libc::splice(
                    pipe_fds[0],
                    std::ptr::null_mut(),
                    dst.as_raw_fd(),
                    std::ptr::null_mut(),
                    (n - written) as usize,
                    libc::SPLICE_F_MOVE | libc::SPLICE_F_MORE,
                )
            };
            if w < 0 {
                break;
            }
            if w == 0 {
                break;
            }
            written += w;
        }

        total += written as u64;
        progress_cb(written as u64);
    };

    unsafe {
        libc::close(pipe_fds[0]);
        libc::close(pipe_fds[1]);
    }

    result
}

/// Standard buffered copy with large buffers
pub fn buffered_copy(
    src: &mut File,
    dst: &mut File,
    _total_size: u64,
    block_size: usize,
    mut progress_cb: impl FnMut(u64),
) -> std::io::Result<u64> {
    let mut buf = vec![0u8; block_size];
    let mut total: u64 = 0;

    advise_sequential(src);

    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])?;
        total += n as u64;
        progress_cb(n as u64);

        // Drop destination cache periodically to avoid memory pressure
        if total % (block_size as u64 * 64) == 0 {
            advise_dontneed(dst, 0, total as i64);
        }
    }

    dst.flush()?;
    Ok(total)
}

/// O_DIRECT copy with aligned buffers
pub fn direct_copy(
    src: &mut File,
    dst: &mut File,
    total_size: u64,
    block_size: usize,
    mut progress_cb: impl FnMut(u64),
) -> std::io::Result<u64> {
    let mut buf = aligned_buffer(block_size);
    let mut total: u64 = 0;

    // Pre-allocate destination
    preallocate(dst, total_size);

    loop {
        let n = src.read(buf.as_mut_slice())?;
        if n == 0 {
            break;
        }

        // O_DIRECT requires aligned writes; the last block may be partial
        if n < block_size {
            let aligned_n = n & !(ALIGNMENT - 1);
            if aligned_n > 0 {
                write_all_at(dst, &buf.as_slice()[..aligned_n])?;
            }
            if n > aligned_n {
                write_all_at(dst, &buf.as_slice()[aligned_n..n])?;
            }
            total += n as u64;
            progress_cb(n as u64);
            continue;
        }

        write_all_at(dst, &buf.as_slice()[..n])?;
        total += n as u64;
        progress_cb(n as u64);
    }

    // Truncate to exact size (O_DIRECT may have written extra)
    dst.set_len(total_size)?;
    Ok(total)
}

fn write_all_at(file: &mut File, buf: &[u8]) -> std::io::Result<()> {
    let mut written = 0;
    while written < buf.len() {
        let n = file.write(&buf[written..])?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "write returned 0",
            ));
        }
        written += n;
    }
    Ok(())
}
