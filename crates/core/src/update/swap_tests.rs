//! The swap of the running program for the new one, and the orderly stop after an install (REQ-UPD-006, 007).
#![allow(clippy::unwrap_used)]

use super::*;
use crate::store::testutil::TempDir;
use std::sync::atomic::{AtomicI32, AtomicUsize};

fn files(tag: &str) -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let d = TempDir::new(tag);
    let exe = d.0.join("clawdot");
    let (new, prev) = (d.0.join("clawdot.new"), d.0.join("clawdot.prev"));
    std::fs::write(&exe, b"old program").unwrap();
    std::fs::write(&new, b"new program").unwrap();
    (d, exe, new, prev)
}

// There is no moment without a program at the path: at every rename the old program is still there, and a failure of
// the second step leaves it as it was.
#[test]
fn the_path_always_holds_a_runnable_program_and_a_failed_second_step_changes_nothing() {
    let (_d, exe, new, prev) = files("swap-fail");
    let steps = AtomicUsize::new(0);
    let r = swap_in(&exe, &new, &prev, &|from, to| {
        steps.fetch_add(1, AtomicOrdering::Relaxed);
        assert_eq!(
            std::fs::read(&exe).unwrap(),
            b"old program",
            "the old program is at its path while {from:?} is renamed to {to:?}"
        );
        if from == new {
            return Err(std::io::Error::other("injected failure of the second step"));
        }
        std::fs::rename(from, to)
    });
    assert!(r.unwrap_err().contains("cannot put the new program"));
    assert_eq!(steps.load(AtomicOrdering::Relaxed), 2);
    assert_eq!(std::fs::read(&exe).unwrap(), b"old program");
    assert!(new.exists(), "the new file is left for the caller to clean");
    assert!(!exe.with_extension("prev-new").exists());
}

#[test]
fn a_swap_keeps_the_old_program_as_prev_and_puts_the_new_one_at_the_path() {
    let (d, exe, new, prev) = files("swap-ok");
    std::fs::write(&prev, b"older program").unwrap();
    swap_in(&exe, &new, &prev, &|from, to| {
        assert_eq!(std::fs::read(&exe).unwrap(), b"old program");
        std::fs::rename(from, to)
    })
    .unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), b"new program");
    assert_eq!(std::fs::read(&prev).unwrap(), b"old program");
    assert!(!new.exists());
    let left: Vec<_> = std::fs::read_dir(&d.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(left.len(), 2, "{left:?}");
}

// The supervisor puts `.prev` back over the program: it must still be the old program after the swap, and it must not
// change when the program at the path is replaced again.
#[cfg(unix)]
#[test]
fn prev_is_a_link_to_the_old_program_not_a_second_copy() {
    use std::os::unix::fs::MetadataExt;
    let (_d, exe, new, prev) = files("swap-link");
    let old_inode = std::fs::metadata(&exe).unwrap().ino();
    swap_in(&exe, &new, &prev, &|f, t| std::fs::rename(f, t)).unwrap();
    assert_eq!(std::fs::metadata(&prev).unwrap().ino(), old_inode);
    assert_ne!(std::fs::metadata(&exe).unwrap().ino(), old_inode);
}

// The whole install: the new program is written and swapped in, and a refused program changes nothing.
#[test]
fn install_program_writes_swaps_and_cleans_up() {
    let (d, exe, new, _prev) = files("swap-install");
    let pending = d.0.join(PENDING_FILE);
    install_program(&exe, Some(&pending), false, b"fresh", "9.9.9", &|f, t| {
        std::fs::rename(f, t)
    })
    .unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), b"fresh");
    assert_eq!(
        std::fs::read(exe.with_file_name("clawdot.prev")).unwrap(),
        b"old program"
    );
    assert_eq!(std::fs::read_to_string(&pending).unwrap(), "9.9.9");
    assert!(!new.exists());

    let (_d2, exe2, new2, _p2) = files("swap-install-fail");
    let e = install_program(&exe2, None, false, b"fresh", "9.9.9", &|f, t| {
        if f == new2 {
            Err(std::io::Error::other("no"))
        } else {
            std::fs::rename(f, t)
        }
    })
    .unwrap_err();
    assert!(e.contains("cannot put the new program"), "{e}");
    assert_eq!(std::fs::read(&exe2).unwrap(), b"old program");
    assert!(!new2.exists(), "the half-installed file is removed");
}

// After an install on a supervised device the updater asks its owner for an orderly stop instead of calling `exit`
// (which would drop writes still queued on the store writer).
#[tokio::test]
async fn an_installed_update_asks_for_an_orderly_stop_instead_of_exiting() {
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    let kp = Ed25519KeyPair::from_pkcs8(
        Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .unwrap()
            .as_ref(),
    )
    .unwrap();
    let key: [u8; 32] = kp.public_key().as_ref().try_into().unwrap();
    let port = super::tests::newer_release(&kp, b"new program", b"new program").await;
    let dir = TempDir::new("update-stop");
    let exe = dir.0.join("clawdot-test-exe");
    std::fs::write(&exe, b"old program").unwrap();
    let cfg = UpdateConfig {
        url: format!("http://127.0.0.1:{port}/feed"),
        public_key: Some(key),
        supervised: true,
    };
    let mut u = Updater::new(cfg, true, Some(dir.0.clone()), exe.clone(), false, true).unwrap();
    let asked = Arc::new(AtomicI32::new(-1));
    let seen = Arc::clone(&asked);
    u.stop = Some(Arc::new(move |code| {
        seen.store(code, AtomicOrdering::SeqCst)
    }));
    u.check().await.unwrap();
    let release = u.lock().latest.clone().unwrap();
    u.apply(&release).await.unwrap();
    assert_eq!(
        asked.load(AtomicOrdering::SeqCst),
        0,
        "the hook was called with the exit code"
    );
    assert_eq!(std::fs::read(&exe).unwrap(), b"new program");
    assert_eq!(u.status()["phase"], "restarting");
}
