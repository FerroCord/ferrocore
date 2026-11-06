use std::process::Command;

fn assert_cli(args: &[&str], code: i32, stdout: &[&str], stderr: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_ferrocore"))
        .args(args)
        .output()
        .expect("ferrocore process should start");
    assert_eq!(output.status.code(), Some(code));
    let actual_stdout = std::str::from_utf8(&output.stdout).expect("stdout is UTF-8");
    let actual_stderr = std::str::from_utf8(&output.stderr).expect("stderr is UTF-8");
    assert_eq!(actual_stdout.lines().collect::<Vec<_>>(), stdout);
    assert_eq!(actual_stderr.lines().collect::<Vec<_>>(), stderr);
}

#[test]
fn check_integrates_ferrocord() {
    assert_cli(&["check"], 0, &["ferrocord integration: ok"], &[]);
}

#[test]
fn usage_without_arguments() {
    assert_cli(&[], 0, &["Usage: ferrocore check"], &[]);
}

#[test]
fn usage_with_help() {
    assert_cli(&["--help"], 0, &["Usage: ferrocore check"], &[]);
}

#[test]
fn unknown_argument_is_rejected() {
    assert_cli(&["unknown"], 2, &[], &["Usage: ferrocore check"]);
}
