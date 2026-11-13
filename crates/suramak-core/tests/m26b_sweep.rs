//! The M26B startup sweep: a `.pcm32` a killed process left behind is removed,
//! and files that are not ours are left alone.
//!
//! A separate test binary on purpose: the sweep deletes every matching temp
//! file in the directory, so it must not run while another test in the same
//! process holds a live mapping.

use suramak_core::pcm::sweep_orphaned_temp_pcm;

#[test]
fn the_startup_sweep_removes_orphans_and_leaves_other_files_alone() {
    let dir = std::env::temp_dir().join(format!("suramak-m26b-sweep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let orphan = dir.join(format!("suramak_{}_orphan.pcm32", std::process::id()));
    let second = dir.join(format!("suramak_{}_second.pcm32", std::process::id()));
    let decoy = dir.join("notes.pcm32");
    let other_ext = dir.join("suramak_1234.log");
    std::fs::write(&orphan, [0u8; 16]).unwrap();
    std::fs::write(&second, [0u8; 16]).unwrap();
    std::fs::write(&decoy, b"not ours").unwrap();
    std::fs::write(&other_ext, b"not ours").unwrap();

    let removed = sweep_orphaned_temp_pcm(&dir);
    assert_eq!(removed, 2, "both planted orphans must be removed");
    assert!(!orphan.exists());
    assert!(!second.exists());
    assert!(decoy.exists(), "a .pcm32 not named suramak_* is not ours");
    assert!(other_ext.exists(), "only .pcm32 files are swept");

    // An absent directory is a no-op (the app may start before temp exists).
    assert_eq!(sweep_orphaned_temp_pcm(&dir.join("missing")), 0);

    let _ = std::fs::remove_dir_all(&dir);
}
