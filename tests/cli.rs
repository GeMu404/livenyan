//! Pruebas de la CLI sin pantalla (--info, --dump-lsb, --trace) sobre fixtures sintéticos.
use std::process::Command;

fn run(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_livenyan")).args(args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn fx(n: &str) -> String {
    format!("{}/tests/fixtures/{n}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn info_counts() {
    let s = run(&["--info", &fx("game.exe")]);
    assert!(s.contains("entries:     5"));
    assert!(s.contains("lsb:         3/3"));
    assert!(s.contains("entry:       ゲームメイン.lsb"));
}

#[test]
fn trace_ends_and_hides_text() {
    let s = run(&["--trace", &fx("game.exe")]);
    assert!(s.contains("state=ended"), "{s}");
    assert!(!s.contains("こんにちは"), "text leaked without --show-text");
    let s = run(&["--trace", "--show-text", &fx("game.dat")]);
    assert!(s.contains("こんにちは"));
}

#[test]
fn dump_hides_text() {
    let s = run(&["--dump-lsb", &fx("game.exe"), "00000001"]);
    assert!(s.contains("TextIns"));
    assert!(!s.contains("こんにちは"));
}
