//! Test harness for the ambient auth posture (OIDC off, fixed node identity,
//! fixture signer registry) that tests of the crates built on `medulla-server-auth` run under.
//!
//! A test binary opts in by depending on this crate (dev-dependency only) and
//! naming it once: `#[cfg(test)] use medulla_test_ambient_auth as _;`. Linking it
//! runs a constructor before `main` that calls
//! `medulla_server_auth::auth::install_ambient_test_posture`. A binary that does not link it keeps the
//! production posture even when Cargo's feature unification compiles the
//! `test-ambient-auth` feature into `medulla-server-auth`, because that feature only
//! makes the switch available; it no longer flips any behaviour on its own.
//! This is why `medulla-engine`'s tests pass in a unified workspace run
//! (MEDULLA-CONTRACT-R056.18).

#[ctor::ctor(unsafe)]
fn install_ambient_test_posture_before_main() {
    medulla_server_auth::auth::install_ambient_test_posture();
}
