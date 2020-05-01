# Release Notes

## Changes in Sawtooth Rust SDK 0.4.3

* Stabilize the "transact-compat" feature by moving the feature flag from the
  "experimental" feature group to the "stable" feature group in the Cargo.toml

## Changes in Sawtooth Rust SDK 0.4.2

* Remove pike and sabre smart permissions namespace from xo manifest

## Changes in Sawtooth Rust SDK 0.4.1

### Highlights

* Add the `new_boxed` method for creating a `Signer` that is not constrained by
  a lifetime

### Experimental Changes

* Add the `TransactSigner` struct, which provides an implementation of the
  Transact library's own `Signer` trait, behind the "transact-compat" feature
