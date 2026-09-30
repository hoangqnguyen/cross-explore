//! A process's current directory, so the file list can follow `cd` typed in
//! the terminal. Best effort: `None` whenever the OS won't tell us.

use std::path::PathBuf;

#[cfg(target_os = "linux")]
pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// `proc_pidinfo(PROC_PIDVNODEPATHINFO)` is the libproc call `lsof` itself
/// uses; it needs no entitlement for our own children.
#[cfg(target_os = "macos")]
pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;

    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: `info` is a properly sized, writable proc_vnodepathinfo.
    let n = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&mut info as *mut libc::proc_vnodepathinfo).cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    // vip_path is a MAXPATHLEN char array declared as [[c_char; 32]; 32].
    let raw = &info.pvi_cdir.vip_path;
    // SAFETY: the nested array is one contiguous MAXPATHLEN-byte buffer.
    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(raw.as_ptr().cast::<u8>(), std::mem::size_of_val(raw))
    };
    let path = CStr::from_bytes_until_nul(bytes).ok()?;
    let path = std::ffi::OsStr::from_bytes(path.to_bytes());
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Windows has no supported way to read another process's current
/// directory (it lives in the target's PEB).
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn process_cwd(_pid: u32) -> Option<PathBuf> {
    None
}
