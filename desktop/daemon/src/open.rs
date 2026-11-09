//! Opening a received file, or showing it in the file manager, on the user's desktop.

use std::{
    path::Path,
    process::{Command, Stdio},
};

use tracing::{debug, warn};

use crate::popups::session_env;

/// `file://` URI for a path, percent-encoding everything but unreserved characters and `/`.
pub fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn spawn(program: &str, arg: &Path) -> bool {
    let mut cmd = Command::new(program);
    cmd.arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in session_env() {
        if std::env::var_os(&k).is_none() {
            cmd.env(k, v);
        }
    }
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(e) => {
            warn!("could not run {program}: {e}");
            false
        }
    }
}

/// Open the file with its default app.
pub fn open_file(path: &Path) -> bool {
    spawn("xdg-open", path)
}

/// Show the file selected in the file manager (Dolphin, Files, …), or at least its folder.
pub fn reveal_file(path: &Path) -> bool {
    let shown = zbus::blocking::Connection::session().and_then(|c| {
        c.call_method(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            Some("org.freedesktop.FileManager1"),
            "ShowItems",
            &(vec![file_uri(path)], ""),
        )
        .map(|_| ())
    });
    match shown {
        Ok(()) => true,
        Err(e) => {
            debug!("file manager did not answer ({e}); opening the folder instead");
            path.parent().is_some_and(|dir| spawn("xdg-open", dir))
        }
    }
}

/// A rough content type from the file name, for the phone to pick an app to open it with.
pub fn guess_mime(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "zip" => "application/zip",
        "apk" => "application/vnd.android.package-archive",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_and_types() {
        assert_eq!(
            file_uri(Path::new("/home/y/Downloads/EchoTwin/My Photo #1.jpg")),
            "file:///home/y/Downloads/EchoTwin/My%20Photo%20%231.jpg"
        );
        assert_eq!(file_uri(Path::new("/tmp/ä.txt")), "file:///tmp/%C3%A4.txt");
        assert_eq!(guess_mime("IMG_2026.JPG"), "image/jpeg");
        assert_eq!(guess_mime("notes"), "application/octet-stream");
        assert!(guess_mime("slides.pptx").contains("presentationml"));
    }
}
