use std::{env, path::PathBuf};

fn main() {
    let profile_env = env::var("PROFILE").unwrap();
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();

    let out_folder = if profile_env == "debug" {
        "debug"
    } else {
        "release"
    };
    let bin_name = if cfg!(windows) {
        "zdiff-gui.exe"
    } else {
        "zdiff-gui"
    };
    let bin_path = workspace_root
        .join("target")
        .join(out_folder)
        .join(bin_name);

    // Warning if you forgot to build the workspace
    if !bin_path.exists() {
        println!(
            "cargo:warning=Sibling binary not found at {}. Did you run 'cargo build --workspace'?",
            bin_path.display()
        );
    }

    println!("cargo:rustc-env=ZDIFF_BIN_PATH={}", bin_path.display());

    // Watch the sibling crate for changes
    println!("cargo:rerun-if-changed=../zdiff/src");
    println!("cargo:rerun-if-changed=../zdiff/Cargo.toml");
}
