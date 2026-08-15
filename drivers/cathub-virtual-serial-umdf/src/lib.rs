// Copyright (c) CatHub contributors.
// SPDX-License-Identifier: MIT

//! Pure Rust UMDF 2 proof of concept for `CatHub` virtual serial endpoints.
//!
//! The current milestone only creates a WDF device. It deliberately does not
//! register a COM port or a private `CatHub` interface until their queues and
//! failure behavior are implemented.

#[cfg_attr(test, allow(dead_code))]
mod interop;
