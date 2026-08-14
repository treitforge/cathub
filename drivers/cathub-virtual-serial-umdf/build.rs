// Copyright (c) CatHub contributors.
// SPDX-License-Identifier: MIT

//! Configure Cargo to link the UMDF driver binary with the WDK.

fn main() -> Result<(), wdk_build::ConfigError> {
    wdk_build::configure_wdk_binary_build()
}
