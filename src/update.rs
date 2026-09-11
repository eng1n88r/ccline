use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "eng1n88r/ccline";

/// Release asset target triple for the running platform, matching the CI matrix.
fn release_target() -> Option<&'static str> {
    Some(match (env::consts::OS, env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        _ => return None,
    })
}

/// `ccline update`: download the latest release binary and replace this
/// executable in place.
pub(crate) fn run() -> Result<(), String> {
    let current = env!("CARGO_PKG_VERSION");
    let tmp = env::temp_dir().join(format!("ccline-update-{}", std::process::id()));
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let _cleanup = Cleanup(tmp.clone());

    // Latest version from the /releases/latest redirect (no API token needed).
    let effective = curl(
        &tmp,
        &[
            "-o",
            &tmp.join("headers").to_string_lossy(),
            "-w",
            "%{url_effective}",
            "-I",
            &format!("https://github.com/{REPO}/releases/latest"),
        ],
    )?;
    let latest = effective
        .trim()
        .rsplit("/tag/v")
        .next()
        .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .ok_or_else(|| format!("cannot parse latest version from {effective:?}"))?
        .to_string();
    if ver_key(&latest) <= ver_key(current) {
        println!("ccline {current} is already the latest release (v{latest})");
        return Ok(());
    }

    let target = release_target().ok_or("no prebuilt release for this platform")?;
    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    let url = format!("https://github.com/{REPO}/releases/latest/download/ccline-{target}.{ext}");
    println!("downloading {url}");
    let archive = tmp.join(format!("ccline.{ext}"));
    curl(&tmp, &["-o", &archive.to_string_lossy(), &url])?;

    // bsdtar (macOS, Windows 10+) and GNU tar both auto-detect the format.
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&tmp)
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    if !status.success() {
        return Err("tar extraction failed".into());
    }

    let new_bin = tmp.join(if cfg!(windows) {
        "ccline.exe"
    } else {
        "ccline"
    });
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    replace_exe(&new_bin, &exe)?;
    println!("updated ccline {current} -> {latest} at {}", exe.display());
    Ok(())
}

/// "1.2.10" -> (1, 2, 10), for ordered version comparison.
fn ver_key(v: &str) -> (u64, u64, u64) {
    let mut parts = v.split('.').map(|p| p.parse().unwrap_or(0));
    let mut next = || parts.next().unwrap_or(0);
    (next(), next(), next())
}

/// Replace `exe` with `new_bin`, handling the running-executable case.
fn replace_exe(new_bin: &Path, exe: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(new_bin, fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    // Stage next to the destination so the final rename is atomic and never
    // crosses filesystems; on Windows the running exe is moved aside first.
    let staged = exe.with_extension("new");
    fs::copy(new_bin, &staged).map_err(|e| format!("staging {}: {e}", staged.display()))?;
    if cfg!(windows) {
        let old = exe.with_extension("old");
        let _ = fs::remove_file(&old);
        fs::rename(exe, &old).map_err(|e| e.to_string())?;
    }
    fs::rename(&staged, exe).map_err(|e| e.to_string())
}

fn curl(tmp: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "60"])
        .args(args)
        .current_dir(tmp)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "curl failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Removes its directory on drop, so temp files go away on every exit path.
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
