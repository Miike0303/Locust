use predicates::prelude::*;

mod common;
use common::locust;

#[test]
fn help_repeatedly_exits_zero_without_stack_overflow() {
    // Exercise the debug executable's OS main thread, not libtest's larger stack.
    for _ in 0..20 {
        for args in [
            &["--help"][..],
            &["translate", "--help"][..],
            &["glossary", "add", "--help"][..],
        ] {
            locust()
                .args(args)
                .assert()
                .code(0)
                .stdout(predicate::str::contains("Usage:"))
                .stderr(predicate::str::is_empty());
        }
    }
}

#[cfg(all(windows, debug_assertions))]
#[test]
fn startup_succeeds_with_a_small_windows_main_thread_stack() {
    // Lower the stack reserve in a private copy of the debug PE executable.
    // This makes dependence on the OS main thread's stack deterministic.
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("locust-small-stack.exe");
    let mut image = std::fs::read(env!("CARGO_BIN_EXE_locust")).unwrap();
    let pe_offset = u32::from_le_bytes(image[0x3c..0x40].try_into().unwrap()) as usize;
    let optional_header = pe_offset + 24;
    let magic = u16::from_le_bytes(
        image[optional_header..optional_header + 2]
            .try_into()
            .unwrap(),
    );
    assert!(matches!(magic, 0x10b | 0x20b), "expected PE32 or PE32+");
    // SizeOfStackReserve starts at offset 72 in both optional header formats.
    // Change its low 32 bits (the high bits in PE32+ are already zero).
    let reserve = optional_header + 72;
    image[reserve..reserve + 4].copy_from_slice(&(256_u32 * 1024).to_le_bytes());
    std::fs::write(&executable, image).unwrap();

    assert_cmd::Command::new(executable)
        .arg("--help")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("Usage:"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn startup_preserves_success_and_error_exit_codes() {
    locust()
        .arg("formats")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("RPG Maker"));

    let dir = tempfile::tempdir().unwrap();
    locust()
        .arg("stats")
        .arg(dir.path().join("missing.locust.db"))
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "Error: project database not found:",
        ));

    locust()
        .arg("--unknown-option")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unexpected argument"));
}
