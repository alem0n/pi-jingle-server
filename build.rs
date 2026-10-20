// Build script.
//
// 1. Copies sounds/ next to the built binary so relative audio paths
//    (sounds/done.mp3) resolve against the executable's directory for both
//    debug and release builds.
// 2. When the `desktop` feature is enabled, runs tauri-build, which validates
//    tauri.conf.json, embeds the Windows manifest/icon and generates the ACL
//    schemas used by the Tauri window.
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=sounds");
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=tauri.conf.json");

    // OUT_DIR = target/<profile>/build/<pkg>-<hash>/out → target/<profile>
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let profile_dir = Path::new(&out_dir)
        .ancestors()
        .nth(3)
        .expect("OUT_DIR has at least 3 ancestors")
        .to_path_buf();

    copy_dir(Path::new("sounds"), &profile_dir.join("sounds"));

    #[cfg(feature = "desktop")]
    tauri_build::build();
}

fn copy_dir(src: &Path, dst: &Path) {
    if !src.is_dir() {
        return;
    }
    fs::create_dir_all(dst).expect("failed to create sounds dir next to binary");
    for entry in fs::read_dir(src).expect("failed to read sounds dir") {
        let entry = entry.expect("failed to read sounds dir entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            fs::copy(&from, &to).expect("failed to copy sound file");
        }
    }
}
