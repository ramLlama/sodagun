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

/// True when this host can boot sandbox VMs (KVM on Linux, Hypervisor.framework
/// on macOS). VM-boot tests skip themselves when this is false — e.g. inside a
/// sodagun sandbox guest, where there is no nested virtualization. The pre-push
/// hook (`scripts/require-virt.sh`, same probe) refuses to push from such hosts
/// so the skipped tests still gate every push.
pub fn has_virtualization() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new("/dev/kvm").exists()
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("sysctl")
            .args(["-n", "kern.hv_support"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        false
    }
}

/// Guard for tests that boot real VMs: returns true (and logs the skip) when
/// the test should be skipped because hardware virtualization is unavailable.
pub fn skip_without_virt(test_name: &str) -> bool {
    if has_virtualization() {
        return false;
    }
    eprintln!("SKIPPED {test_name}: no hardware virtualization, cannot boot sandbox VMs");
    true
}
