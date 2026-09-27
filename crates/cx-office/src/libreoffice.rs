//! LibreOffice as a high-fidelity renderer: `soffice --headless
//! --convert-to pdf` produces exactly what the office suite would print.
//!
//! Only on desktop platforms; phones can't run external programs, so there
//! [`detect`] finds nothing and every preview uses the built-in renderers.
//!
//! LibreOffice refuses to run twice against one user profile, and a user's
//! own running instance would otherwise swallow our request, so conversions
//! use a dedicated profile (`-env:UserInstallation`) under the cache
//! directory and are serialized by the caller.

use cx_core::Result;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Find LibreOffice: `CX_SOFFICE` if set, then `soffice`/`libreoffice` on
/// `PATH`, then the standard install locations.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub(crate) fn detect() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CX_SOFFICE").map(PathBuf::from) {
        return p.is_file().then_some(p);
    }
    let names: &[&str] = if cfg!(windows) { &["soffice.exe", "soffice.com", "libreoffice.exe"] } else { &["soffice", "libreoffice"] };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for n in names {
                let p = dir.join(n);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    let fixed: &[&str] = &[
        "/Applications/LibreOffice.app/Contents/MacOS/soffice",
        r"C:\Program Files\LibreOffice\program\soffice.exe",
        r"C:\Program Files (x86)\LibreOffice\program\soffice.exe",
        "/usr/bin/soffice",
        "/usr/local/bin/soffice",
        "/usr/lib/libreoffice/program/soffice",
        "/opt/libreoffice/program/soffice",
        "/snap/bin/libreoffice",
    ];
    let mut candidates: Vec<PathBuf> = fixed.iter().map(PathBuf::from).collect();
    if let Some(home) = cx_core::location::home_dir() {
        candidates.push(home.join("Applications/LibreOffice.app/Contents/MacOS/soffice"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) fn detect() -> Option<PathBuf> {
    None
}

/// Convert `input` to PDF in `out_dir` and return the PDF's path. Blocking;
/// kills LibreOffice if it takes longer than `timeout`.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub(crate) fn convert(soffice: &Path, input: &Path, profile: &Path, out_dir: &Path, timeout: Duration) -> Result<PathBuf> {
    use cx_core::CxError;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    std::fs::create_dir_all(profile).map_err(|e| CxError::from_io(e, profile.display()))?;
    let profile_url = url::Url::from_directory_path(profile).map_err(|_| CxError::Io(format!("bad profile path {}", profile.display())))?;
    let mut cmd = Command::new(soffice);
    cmd.arg(format!("-env:UserInstallation={profile_url}"))
        .args(["--headless", "--invisible", "--norestore", "--nolockcheck", "--nodefault", "--nologo", "--convert-to", "pdf", "--outdir"])
        .arg(out_dir)
        .arg(input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| CxError::io(format!("starting {}", soffice.display()), e))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CxError::Io(format!("LibreOffice took longer than {}s", timeout.as_secs())));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(CxError::io("waiting for LibreOffice", e)),
        }
    };
    let stem = input.file_stem().map(|s| s.to_os_string()).unwrap_or_default();
    let pdf = out_dir.join(Path::new(&stem).with_extension("pdf"));
    if pdf.is_file() {
        return Ok(pdf);
    }
    // Some versions normalize the output name; take any PDF produced.
    if let Ok(rd) = std::fs::read_dir(out_dir) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) {
                return Ok(e.path());
            }
        }
    }
    Err(CxError::Io(format!("LibreOffice could not convert {} (exit status {status})", input.display())))
}

#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) fn convert(_: &Path, input: &Path, _: &Path, _: &Path, _: Duration) -> Result<PathBuf> {
    Err(cx_core::CxError::Unsupported(format!("no LibreOffice on this platform to convert {}", input.display())))
}
