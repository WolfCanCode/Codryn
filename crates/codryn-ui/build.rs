use std::path::Path;
use std::process::Command;

fn main() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let ui_dir = workspace_root.join("ui");
    let dist_dir = ui_dir.join("dist");

    println!("cargo:rerun-if-changed={}", ui_dir.join("src").display());
    println!(
        "cargo:rerun-if-changed={}",
        ui_dir.join("package.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        ui_dir.join("vite.config.ts").display()
    );

    // Skip UI build if SKIP_UI_BUILD is set (for lint/test CI jobs)
    if std::env::var("SKIP_UI_BUILD").is_ok() {
        if !dist_dir.exists() {
            std::fs::create_dir_all(&dist_dir).ok();
            std::fs::write(dist_dir.join("index.html"), "<html></html>").ok();
        }
        return;
    }

    // Install node_modules if needed
    if !ui_dir.join("node_modules").exists() {
        let status = Command::new("npm")
            .arg("install")
            .current_dir(&ui_dir)
            .status()
            .expect("failed to run npm install");
        assert!(status.success(), "npm install failed");
    }

    // Build the React app
    let status = Command::new("npm")
        .args(["run", "build"])
        .current_dir(&ui_dir)
        .status()
        .expect("failed to run npm run build");
    assert!(status.success(), "npm run build failed");

    assert!(
        dist_dir.exists(),
        "Vite build output not found at {}",
        dist_dir.display()
    );
}
