//! Integration test entry point.
//!
//! `automod::dir!` pulls every `.rs` file under `tests/integration/` in as a
//! submodule, so new test files are picked up automatically with no Cargo.toml
//! `[[test]]` registration. All integration tests share this single binary.
//!
//! The macro is wrapped in an inline `mod integration` so the generated `mod
//! <file>;` items resolve against `tests/integration/` (a crate-root file
//! resolves bare submodules against `tests/` itself).

mod integration {
    automod::dir!("tests/integration");
}
