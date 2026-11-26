// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the Tessera extensions share.
//!
//! The bindings of `wit/extension.wit` are generated here once and
//! re-exported, so an extension needs no `wit_bindgen::generate!` of its
//! own. An extension implements [`Extension`] for a type holding its state
//! and hands the type to [`extension!`], which keeps one value of it and
//! answers Tessera with it:
//!
//! ```ignore
//! use tessera_extension_kit::{self as kit, Extension, Ui};
//!
//! struct Hello;
//!
//! impl Extension for Hello {
//!     fn start(_now: u64) -> Self {
//!         Hello
//!     }
//!
//!     fn render(&mut self, _widget: &str, _now: u64) -> Ui {
//!         let mut ui = Ui::bar();
//!         ui.text(Ui::ROOT, "Hello");
//!         ui
//!     }
//! }
//!
//! kit::extension!(Hello);
//! ```
//!
//! Beside the bindings the kit has a builder for the trees `render`
//! returns ([`Ui`]), readers for settings ([`settings`]), a store for the
//! state kept across restarts ([`store`]), requests on a timer and in a
//! queue ([`net`]), calendar and clock arithmetic ([`time`]), number and
//! text formatting ([`format`]) and a small random number generator
//! ([`rng`]).

/// The bindings of `wit/extension.wit`, also re-exported at the root.
pub mod bindings {
    wit_bindgen::generate!({
        world: "extension",
        path: "../../wit",
        pub_export_macro: true,
        export_macro_name: "export",
        default_bindings_module: "tessera_extension_kit::bindings",
    });
}

pub use bindings::*;

/// Writes `message` to the shell's log; to the standard error when run
/// natively, as in tests, where there is no shell.
pub fn log(message: &str) {
    #[cfg(target_arch = "wasm32")]
    bindings::log(message);
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("{message}");
}

pub mod format;
pub mod net;
pub mod rng;
pub mod settings;
pub mod store;
pub mod time;
mod ui;

pub use ui::{Id, Ui};

/// An extension: the state it keeps between calls and how it answers
/// Tessera. [`extension!`] creates the value on the first call.
pub trait Extension: Sized + 'static {
    /// The state before the first call, `now` seconds after 1970 began.
    fn start(now: u64) -> Self;

    /// The current content of `widget`, one of the widgets of the
    /// manifest.
    fn render(&mut self, widget: &str, now: u64) -> Ui;

    /// The user pressed the button of `widget` whose action is `action`.
    fn activate(&mut self, _widget: &str, _action: &str, _now: u64) {}

    /// The outcome of the request `fetch` numbered `request`.
    fn fetched(&mut self, _request: u32, _outcome: Result<Response, String>, _now: u64) {}
}

/// Exports the [`Extension`] `$app` to Tessera, keeping one value of it
/// for the life of the extension.
#[macro_export]
macro_rules! extension {
    ($app:ty) => {
        const _: () = {
            ::std::thread_local! {
                static APP: ::core::cell::RefCell<::core::option::Option<$app>> =
                    const { ::core::cell::RefCell::new(::core::option::Option::None) };
            }

            fn with_app<T>(run: impl FnOnce(&mut $app, u64) -> T) -> T {
                let now = $crate::now();
                APP.with_borrow_mut(|app| {
                    let app = app.get_or_insert_with(|| <$app as $crate::Extension>::start(now));
                    run(app, now)
                })
            }

            struct Exported;

            impl $crate::Guest for Exported {
                fn render(widget: ::std::string::String) -> ::std::vec::Vec<$crate::Node> {
                    with_app(|app, now| $crate::Extension::render(app, &widget, now).into_nodes())
                }

                fn activate(widget: ::std::string::String, action: ::std::string::String) {
                    with_app(|app, now| $crate::Extension::activate(app, &widget, &action, now))
                }

                fn fetched(
                    request: u32,
                    outcome: ::core::result::Result<$crate::Response, ::std::string::String>,
                ) {
                    with_app(|app, now| $crate::Extension::fetched(app, request, outcome, now))
                }
            }

    $crate::bindings::export!(Exported with_types_in $crate::bindings);
        };
    };
}
