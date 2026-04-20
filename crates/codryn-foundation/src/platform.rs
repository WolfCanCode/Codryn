/// Platform detection utilities.
pub fn home_dir() -> Option<String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
}

pub fn is_windows() -> bool {
    cfg!(target_os = "windows")
}

pub fn is_macos() -> bool {
    cfg!(target_os = "macos")
}

pub fn normalize_path_sep(path: &mut String) {
    if cfg!(target_os = "windows") {
        *path = path.replace('\\', "/");
    }
}
