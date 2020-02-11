# Release Notes

## Changes in Sawtooth Rust SDK 0.4.1

### Highlights

* Add the `new_boxed` method for creating a `Signer` that is not constrained by
  a lifetime

### Experimental Changes

* Add the `TransactSigner` struct, which provides an implementation of the
  Transact library's own `Signer` trait, behind the "transact-compat" feature
