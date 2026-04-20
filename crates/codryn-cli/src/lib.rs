pub mod doctor;
pub mod install;
pub mod update;
pub mod version;

static mut VERSION: Option<String> = None;

pub fn set_version(ver: &str) {
    unsafe {
        VERSION = Some(ver.to_owned());
    }
}

pub fn get_version() -> &'static str {
    #[allow(static_mut_refs)]
    unsafe {
        VERSION.as_deref().unwrap_or("dev")
    }
}
