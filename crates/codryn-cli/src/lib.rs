pub mod activate;
pub mod backup;
pub mod complexity;
pub mod dedupe;
pub mod deps;
pub mod doc_coverage;
pub mod doctor;
pub mod index_runs;
pub mod install;
pub mod mcp_config;
pub mod preferences;
pub mod prompter;
pub mod query;
pub mod query_tool;
pub mod snapshots;
pub mod steering;
pub mod uninstall;
pub mod update;
pub mod validate;
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
