//! Free space on the filesystem that holds a path.

use std::{io, path::Path};

/// Bytes an unprivileged process can still write on the filesystem holding `path`.
#[cfg(unix)]
pub fn available(path: &Path) -> io::Result<u64> {
    let stats = rustix::fs::statvfs(path)?;

    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

/// Bytes the calling user can still write on the volume holding `path`.
#[cfg(windows)]
pub fn available(path: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide = path.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
    let mut free = 0_u64;

    // SAFETY: `wide` is a NUL-terminated path, and the output pointer is a valid u64.
    let result = unsafe {
        GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut())
    };

    if result == 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(free)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_free_space_for_existing_paths_only() {
        let directory = std::env::temp_dir();

        assert!(available(&directory).unwrap() > 0);
        assert!(available(&directory.join("krabb-missing/nested")).is_err());
    }
}
