//! URL parsing extension for `lotus-core`.
//!
//! Registers a catalogue of scalar formula functions for inspecting and
//! manipulating URL strings (`URL_SCHEME`, `URL_HOST`, `URL_PATH_SEGMENT`,
//! `URL_PARAM`, `URL_ENCODE`, `URL_JOIN`, …). URLs are kept as plain text
//! cell values — there's no custom cell type, so URL columns just look
//! like strings to the rest of the engine.
//!
//! ## Usage
//!
//! ```no_run
//! use std::sync::Arc;
//! use lotus_core::{Registry, Sheet};
//!
//! let mut reg = Registry::new();
//! lotus_url::register(&mut reg).expect("registry is empty");
//! let mut sheet = Sheet::new_with_registry(Arc::new(reg));
//! sheet.set_cells(&[
//!     ("A1".into(), "https://www.instagram.com/localfixture/".into()),
//!     ("B1".into(), "=URL_PATH_SEGMENT(A1, -1)".into()),
//! ]).unwrap();
//! ```

mod functions;

use std::sync::Arc;

use lotus_core::{Registry, RegistryError, Sheet};

/// Register every URL function on `registry`.
///
/// Errors if any function name collides with an existing built-in or
/// previously-registered custom function.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    functions::register_url_functions(registry)
}

/// Convenience for binding crates that hold a `Sheet` instead of a raw
/// `Registry`. Mirrors `lotus_datetime::register_on_sheet`.
pub fn register_on_sheet(sheet: &mut Sheet) -> Result<(), RegistryError> {
    for func in function_arcs() {
        sheet.register_function(func)?;
    }
    Ok(())
}

/// Every formula function this crate ships, in registration order.
fn function_arcs() -> Vec<Arc<dyn lotus_core::CustomFunction>> {
    let mut tmp = Registry::new();
    let _ = functions::register_url_functions(&mut tmp);
    tmp.into_functions()
}
