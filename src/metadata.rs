use std::fs;

use std::path::Path;

use filetime::FileTime;

use crate::error::Result;

/// Copy metadata (permissions, timestamps) from source to destination
pub fn copy_metadata(src: &Path, dst: &Path) -> Result<()> {
    let src_meta = fs::metadata(src)?;

    // Copy permissions
    let permissions = src_meta.permissions();
    fs::set_permissions(dst, permissions)?;

    // Copy timestamps
    let atime = FileTime::from_last_access_time(&src_meta);
    let mtime = FileTime::from_last_modification_time(&src_meta);
    filetime::set_file_times(dst, atime, mtime)?;

    // Try to copy ownership (requires root)
    copy_ownership(src, dst);

    Ok(())
}

fn copy_ownership(src: &Path, dst: &Path) {
    use std::os::unix::fs::MetadataExt;
    if let Ok(meta) = fs::metadata(src) {
        let uid = meta.uid();
        let gid = meta.gid();
        // Best effort - will fail if not root
        unsafe {
            let dst_cstr = std::ffi::CString::new(dst.to_string_lossy().as_bytes()).unwrap();
            libc::chown(dst_cstr.as_ptr(), uid, gid);
        }
    }
}

/// Check if two files have the same mtime and size
pub fn same_mtime_size(src: &Path, dst: &Path) -> std::io::Result<bool> {
    let src_meta = fs::metadata(src)?;
    let dst_meta = fs::metadata(dst)?;

    let src_mtime = FileTime::from_last_modification_time(&src_meta);
    let dst_mtime = FileTime::from_last_modification_time(&dst_meta);

    Ok(src_meta.len() == dst_meta.len() && src_mtime == dst_mtime)
}
