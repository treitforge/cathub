// Copyright (c) CatHub contributors.
// SPDX-License-Identifier: MIT

//! Pure Rust UMDF 2 driver for `CatHub`-managed virtual serial endpoints.
//!
//! Each device exposes one public Windows COM port and one ACL-restricted,
//! reference-named private interface used by the `CatHub` daemon. The driver
//! owns bounded bidirectional queues and implements the Windows serial control
//! surface exercised by the native conformance harness.

#[cfg_attr(test, allow(dead_code))]
mod data_plane;
#[cfg_attr(test, allow(dead_code))]
mod interop;
#[cfg_attr(test, allow(dead_code))]
mod private_protocol;
#[cfg_attr(test, allow(dead_code))]
mod serial;
