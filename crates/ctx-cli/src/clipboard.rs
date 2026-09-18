//! Clipboard access for `ctx pack --clip` and `ctx save --paste`.
//!
//! The clipboard is the ground-truth protocol for browser surfaces (spec
//! §10.2): if the extension breaks, the user loses two keystrokes, not the
//! workflow.

use anyhow::{Context, Result};

/// On Linux a process-owned clipboard dies with the process, so prefer the
/// desktop's own tools there, which hand the contents to a long-lived owner.
#[cfg(target_os = "linux")]
fn linux_copy(text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let tools: [(&str, &[&str]); 3] = [
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (bin, args) in tools {
        let Ok(mut child) = Command::new(bin).args(args).stdin(Stdio::piped()).spawn() else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        if child.wait().is_ok_and(|s| s.success()) {
            return true;
        }
    }
    false
}

pub fn copy(text: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    if linux_copy(text) {
        return Ok(());
    }
    let mut cb = arboard::Clipboard::new().context("opening the clipboard")?;
    cb.set_text(text.to_owned())
        .context("writing to the clipboard")?;
    Ok(())
}

pub fn paste() -> Result<String> {
    let mut cb = arboard::Clipboard::new().context("opening the clipboard")?;
    cb.get_text().context("reading text from the clipboard")
}
