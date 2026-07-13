//! Rust reference implementation of UOSC's portable kernel logic.
//!
//! This crate ports the capability model, physical/virtual memory allocator,
//! scheduler, and IPC subsystem specified in `../kernel/*.ti` into real,
//! tested Rust. It exists because the Titan specification currently has no
//! compiler backend that produces a bootable binary, and several of its
//! helper functions are stubs (`Ok(())`, `/* ... */`) rather than working
//! logic. Where this port differs from the Titan source in behavior, the
//! module-level docs say so explicitly and why.
//!
//! This is portable kernel *logic*, not a bootable kernel: no boot sequence,
//! no hardware page tables, no interrupt handling. Those require a
//! hardware-specific bring-up effort out of scope for this pass. What's here
//! is real, runs under `cargo test`, and is meant to be the trusted core
//! that a future hardware layer sits on top of.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod capability;
pub mod ipc;
pub mod memory;
pub mod scheduler;
