//! Check (and optionally install) the host tools this project expects, as
//! documented in the README's "Host setup" section: ffmpeg, aplay, and the
//! piper voice models. (The `piper` binary itself is a separate install —
//! see the README — this script doesn't manage it.)
//!
//! Standalone by design: no dependency on this repo's `src/` crate and no
//! external crates, so it can be copied out and run on its own with plain
//! `rustc`. The paths/URLs below are kept in sync with `src/say.rs` by
//! hand; see the README's "Host setup" table for what they should match.
//!
//! ffmpeg and aplay are looked up on `PATH` and, if missing, installed via
//! `dnf` (Fedora only).
//!
//! Usage:
//!     rustc --edition 2024 scripts/setup_piper_voices.rs -o /tmp/setup_piper_voices
//!     /tmp/setup_piper_voices             # check only
//!     /tmp/setup_piper_voices --install   # fetch/install what's missing

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const HF_BASE_URL: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/main";
const USER_AGENT: &str = "my-cli-setup-piper-voices";

/// Relative to the voices root, mirroring the rhasspy/piper-voices
/// HuggingFace layout.
const VOICE_FILES: &[&str] = &[
    "en/en_US/amy/medium/en_US-amy-medium.onnx",
    "en/en_US/amy/medium/en_US-amy-medium.onnx.json",
    "ru/ru_RU/denis/medium/ru_RU-denis-medium.onnx",
    "ru/ru_RU/denis/medium/ru_RU-denis-medium.onnx.json",
    "ru/ru_RU/irina/medium/ru_RU-irina-medium.onnx",
    "ru/ru_RU/irina/medium/ru_RU-irina-medium.onnx.json",
];

/// Command name -> dnf package that provides it.
const SYSTEM_PACKAGES: &[(&str, &str)] = &[("ffmpeg", "ffmpeg-free"), ("aplay", "alsa-utils")];

fn voices_root() -> Result<PathBuf, String> {
    let home = env::var_os("HOME").ok_or("HOME environment variable is not set")?;
    Ok(PathBuf::from(home).join(".local/share/piper-voices"))
}

fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

/// Look up `name` on `$PATH`, the way `shutil.which` does.
fn which(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_file(candidate))
}

fn check_voice_files(root: &Path) -> Vec<(PathBuf, bool)> {
    VOICE_FILES
        .iter()
        .map(|relative| {
            let path = root.join(relative);
            let exists = path.exists();
            (path, exists)
        })
        .collect()
}

fn check_system_tools() -> Vec<(&'static str, Option<PathBuf>)> {
    SYSTEM_PACKAGES.iter().map(|&(name, _)| (name, which(name))).collect()
}

fn package_for(name: &str) -> &'static str {
    SYSTEM_PACKAGES
        .iter()
        .find(|&&(candidate, _)| candidate == name)
        .map(|&(_, package)| package)
        .expect("name is always one of SYSTEM_PACKAGES's keys")
}

/// Print a status line per path/tool; return true if everything is present.
fn report(voice_status: &[(PathBuf, bool)], tool_status: &[(&str, Option<PathBuf>)]) -> bool {
    for (name, resolved) in tool_status {
        match resolved {
            Some(path) => println!("ok       {name} -> {}", path.display()),
            None => println!("MISSING  {name} -> dnf install -y {}", package_for(name)),
        }
    }
    for (path, present) in voice_status {
        println!("{}  {}", if *present { "ok     " } else { "MISSING" }, path.display());
    }
    tool_status.iter().all(|(_, resolved)| resolved.is_some())
        && voice_status.iter().all(|(_, present)| *present)
}

fn install_missing_system_tools(tool_status: &[(&str, Option<PathBuf>)]) -> Result<(), String> {
    for (name, resolved) in tool_status {
        if resolved.is_none() {
            let package = package_for(name);
            println!("Installing {package} (provides `{name}`) ...");
            let status = Command::new("sudo")
                .args(["dnf", "install", "-y", package])
                .status()
                .map_err(|err| format!("failed to run sudo dnf install: {err}"))?;
            if !status.success() {
                return Err(format!("dnf install -y {package} failed ({status})"));
            }
        }
    }
    Ok(())
}

/// Download `path` from the HuggingFace voices repo, mirroring `root`'s layout.
fn download_voice_file(path: &Path, root: &Path) -> Result<(), String> {
    let relative = path.strip_prefix(root).expect("path is under the voices root");
    let url = format!("{HF_BASE_URL}/{}", relative.to_string_lossy().replace('\\', "/"));
    println!("Downloading {url} -> {}", path.display());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
    }
    let status = Command::new("curl")
        .args(["-fsSL", "-A", USER_AGENT, "-o"])
        .arg(path)
        .arg(&url)
        .status()
        .map_err(|err| format!("failed to run curl (is it installed?): {err}"))?;
    if !status.success() {
        return Err(format!("curl failed downloading {url} ({status})"));
    }
    Ok(())
}

fn install_missing_voice_files(voice_status: &[(PathBuf, bool)], root: &Path) -> Result<(), String> {
    for (path, present) in voice_status {
        if !present {
            download_voice_file(path, root)?;
        }
    }
    Ok(())
}

fn parse_args(args: &[String]) -> Result<bool, String> {
    match args {
        [] => Ok(false),
        [flag] if flag == "--install" => Ok(true),
        _ => Err("usage: setup_piper_voices [--install]".to_string()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let install = match parse_args(&args) {
        Ok(install) => install,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };

    let root = match voices_root() {
        Ok(root) => root,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };

    let mut voice_status = check_voice_files(&root);
    let mut tool_status = check_system_tools();
    let already_complete = tool_status.iter().all(|(_, r)| r.is_some())
        && voice_status.iter().all(|(_, present)| *present);

    if install && !already_complete {
        if let Err(message) = install_missing_system_tools(&tool_status) {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
        if let Err(message) = install_missing_voice_files(&voice_status, &root) {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
        voice_status = check_voice_files(&root);
        tool_status = check_system_tools();
    }

    if report(&voice_status, &tool_status) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_defaults_to_no_install() {
        assert_eq!(parse_args(&[]), Ok(false));
    }

    #[test]
    fn parse_args_recognizes_install_flag() {
        assert_eq!(parse_args(&["--install".to_string()]), Ok(true));
    }

    #[test]
    fn parse_args_rejects_unknown_argument() {
        assert!(parse_args(&["--bogus".to_string()]).is_err());
    }

    #[test]
    fn package_for_maps_known_tools() {
        assert_eq!(package_for("ffmpeg"), "ffmpeg-free");
        assert_eq!(package_for("aplay"), "alsa-utils");
    }

    #[test]
    fn which_finds_a_binary_known_to_exist_on_any_linux_host() {
        // `sh` is POSIX-mandated, so this is a deterministic positive case
        // without depending on ffmpeg/aplay/piper being installed.
        assert!(which("sh").is_some());
    }

    #[test]
    fn which_returns_none_for_a_nonexistent_binary() {
        assert!(which("definitely-not-a-real-binary-xyz").is_none());
    }

    #[test]
    fn is_executable_file_false_for_missing_path() {
        assert!(!is_executable_file(Path::new("/nonexistent/setup-piper-voices-test/nope")));
    }

    #[test]
    fn is_executable_file_true_for_a_binary_on_path() {
        let sh = which("sh").expect("sh must exist for this test to be meaningful");
        assert!(is_executable_file(&sh));
    }

    #[test]
    fn check_voice_files_reports_all_missing_under_an_empty_root() {
        let root = PathBuf::from("/nonexistent/setup-piper-voices-test-root");
        let status = check_voice_files(&root);
        assert_eq!(status.len(), VOICE_FILES.len());
        assert!(status.iter().all(|(_, present)| !present));
    }
}
