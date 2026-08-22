// Copyright (c) CatHub contributors.
// SPDX-License-Identifier: MIT

//! Pure Rust UMDF 2 proof of concept for `CatHub` virtual serial endpoints.
//!
//! The current milestone creates a private, reference-named application/daemon
//! interface with bounded bidirectional queues. It deliberately does not yet
//! register a public COM port.

#[cfg_attr(test, allow(dead_code))]
mod data_plane;
#[cfg_attr(test, allow(dead_code))]
mod interop;
#[cfg_attr(test, allow(dead_code))]
mod private_protocol;
#[cfg_attr(test, allow(dead_code))]
mod serial;
