//! End-to-end tests for the signed-package flow, exercised through the public
//! API only. Lives here (integration) rather than as inline unit tests so the
//! test binary's name doesn't trip Windows installer-detection (see Cargo.toml).

use scribe_core::config::UpdateConfig;
use scribe_update::{generate_keypair, sign_package, verify_package, Package};

fn cfg_with(pubkey: String) -> UpdateConfig {
    UpdateConfig {
        enabled: true,
        public_key: Some(pubkey),
        allow_downgrade: true,
        allow_target_mismatch: true,
        ..UpdateConfig::default()
    }
}

#[test]
fn sign_read_verify_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("scribe-bin");
    std::fs::write(&bin, b"#!/bin/sh\necho fake\n").unwrap();
    let pkg_path = dir.path().join("pkg.tar.gz");

    let key = generate_keypair();
    let manifest = sign_package(
        &key,
        &bin,
        "9.9.9",
        Some("any"),
        "notes",
        "2026-06-14T00:00:00Z",
        &pkg_path,
    )
    .unwrap();
    assert_eq!(manifest.name, "scribe");
    assert_eq!(manifest.version, "9.9.9");

    let pkg = Package::read(&pkg_path).unwrap();
    verify_package(&cfg_with(key.public_hex()), &pkg).expect("valid package verifies");
}

#[test]
fn wrong_key_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("b");
    std::fs::write(&bin, b"binary").unwrap();
    let pkg_path = dir.path().join("pkg.tar.gz");

    let signer = generate_keypair();
    sign_package(&signer, &bin, "9.9.9", Some("any"), "", "t", &pkg_path).unwrap();
    let pkg = Package::read(&pkg_path).unwrap();

    let attacker = generate_keypair();
    assert!(verify_package(&cfg_with(attacker.public_hex()), &pkg).is_err());
}

#[test]
fn tampered_binary_fails_checksum() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("b");
    std::fs::write(&bin, b"binary").unwrap();
    let pkg_path = dir.path().join("pkg.tar.gz");

    let key = generate_keypair();
    sign_package(&key, &bin, "9.9.9", Some("any"), "", "t", &pkg_path).unwrap();

    let mut pkg = Package::read(&pkg_path).unwrap();
    pkg.binary.push(0xFF); // tamper after signing
    assert!(verify_package(&cfg_with(key.public_hex()), &pkg).is_err());
}

#[test]
fn downgrade_is_blocked_unless_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("b");
    std::fs::write(&bin, b"binary").unwrap();
    let pkg_path = dir.path().join("pkg.tar.gz");

    let key = generate_keypair();
    // version 0.0.1 is <= the running version → a downgrade.
    sign_package(&key, &bin, "0.0.1", Some("any"), "", "t", &pkg_path).unwrap();
    let pkg = Package::read(&pkg_path).unwrap();

    let mut cfg = cfg_with(key.public_hex());
    cfg.allow_downgrade = false;
    assert!(verify_package(&cfg, &pkg).is_err(), "downgrade must be blocked");

    cfg.allow_downgrade = true;
    assert!(verify_package(&cfg, &pkg).is_ok(), "downgrade allowed with flag");
}

#[test]
fn target_mismatch_is_blocked_unless_allowed() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("b");
    std::fs::write(&bin, b"binary").unwrap();
    let pkg_path = dir.path().join("pkg.tar.gz");

    let key = generate_keypair();
    sign_package(
        &key,
        &bin,
        "9.9.9",
        Some("sparc64-unknown-weird"),
        "",
        "t",
        &pkg_path,
    )
    .unwrap();
    let pkg = Package::read(&pkg_path).unwrap();

    let mut cfg = cfg_with(key.public_hex());
    cfg.allow_target_mismatch = false;
    assert!(verify_package(&cfg, &pkg).is_err(), "foreign target blocked");

    cfg.allow_target_mismatch = true;
    assert!(verify_package(&cfg, &pkg).is_ok());
}
