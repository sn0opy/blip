//! Moving files to the system trash.

use std::path::PathBuf;

/// One result per file.
pub fn move_to_trash(files: &[PathBuf]) -> Vec<Result<(), String>> {
    #[cfg(target_os = "linux")]
    if std::path::Path::new("/.flatpak-info").exists() {
        return portal::move_to_trash(files);
    }
    match trash::delete_all(files) {
        Ok(()) => files.iter().map(|_| Ok(())).collect(),
        // The error doesn't say which files are left.
        Err(e) => (files.iter())
            .map(|f| {
                if f.exists() {
                    Err(e.to_string())
                } else {
                    Ok(())
                }
            })
            .collect(),
    }
}

/// The Flatpak sandbox points `XDG_DATA_HOME` into the app's own folder, so only the
/// portal reaches the user's trash.
#[cfg(target_os = "linux")]
mod portal {
    use std::path::{Path, PathBuf};
    use zbus::blocking::{Connection, Proxy};

    pub fn move_to_trash(files: &[PathBuf]) -> Vec<Result<(), String>> {
        match trash_portal() {
            Ok(portal) => files.iter().map(|f| trash_file(&portal, f)).collect(),
            Err(e) => files.iter().map(|_| Err(e.to_string())).collect(),
        }
    }

    fn trash_portal() -> zbus::Result<Proxy<'static>> {
        Proxy::new(
            &Connection::session()?,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Trash",
        )
    }

    fn trash_file(portal: &Proxy, path: &Path) -> Result<(), String> {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let fd = zbus::zvariant::Fd::from(&file);
        let trashed: u32 = (portal.call("TrashFile", &(fd,))).map_err(|e| e.to_string())?;
        if trashed == 1 {
            Ok(())
        } else {
            Err("the trash portal refused".into())
        }
    }
}
