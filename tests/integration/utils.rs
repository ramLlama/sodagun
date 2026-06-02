use assert_cmd::Command;
use tempfile::TempDir;

pub fn sodagun() -> Command {
    Command::cargo_bin("sodagun").unwrap()
}

/// Like `sodagun()` but with `XDG_CONFIG_HOME` pointing to an empty temp dir,
/// isolating the test from the real `~/.config/sodagun/` user config.
pub fn sodagun_isolated(xdg_tmp: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("sodagun").unwrap();
    cmd.env("XDG_CONFIG_HOME", xdg_tmp.path());
    cmd
}
