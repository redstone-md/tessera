// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::error::Error;

/// The application composition root: platform facts become a portable UI view.
#[cfg(windows)]
pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    tessera_ui::run(|| {
        let snapshot = tessera_windows::observe().map_err(|error| error.to_string())?;
        let titles = snapshot
            .windows()
            .iter()
            .map(|window| window.title().to_owned())
            .collect();
        Ok(tessera_ui::PanelSnapshot::new(
            snapshot.monitors().len(),
            titles,
            snapshot.warnings().len(),
        ))
    })?;
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    Err(tessera_windows::ObservationError::UnsupportedPlatform.into())
}
