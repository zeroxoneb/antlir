/*
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * This source code is licensed under both the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree and the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree.
 */

//! Defines the [set_panichandler] function that wraps around
//! [std::panic::set_hook] to make it easier to define a handler for panics

#![deny(warnings, missing_docs, clippy::all, broken_intra_doc_links)]

use std::io::{self, BufWriter, Write};
use std::panic::{self, PanicInfo};
use std::ptr;

use backtrace::{Backtrace, SymbolName};

const MAX_FRAMES: usize = 1000;

// Enable (and remove) once coredumper can deal with inline entries of the form
//   -> file:line symbol
const INLINED_NAME: bool = false;

/// What's the fate of the process when a panic happens?
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Fate {
    /// Carry on, stiff upper lip old chap
    Continue,
    /// Exit with the given exit code
    Exit(i32),
    /// Kill self with SIGABRT
    Abort,
}

impl Default for Fate {
    fn default() -> Self {
        Fate::Continue
    }
}

fn handler(panic: &PanicInfo<'_>, fate: Fate) {
    let stderr = io::stderr();
    let mut w = BufWriter::new(stderr.lock());

    let payload = panic.payload();
    let msg: &str = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
        .unwrap_or("(about something)");

    let _ = writeln!(w, "PANIC: {}", msg);
    if let Some(loc) = panic.location() {
        let _ = writeln!(w, "from {}", loc);
    }

    let bt = Backtrace::new();
    let frames = bt.frames();
    let frames = if frames.len() > MAX_FRAMES {
        let _ = writeln!(w, "(limiting {} frames to {})", frames.len(), MAX_FRAMES);
        &frames[..MAX_FRAMES]
    } else {
        frames
    };

    for f in frames {
        for (i, symbol) in f.symbols().iter().enumerate() {
            let name = symbol.name().unwrap_or_else(|| SymbolName::new(b"<???>"));
            let addr = symbol.addr().unwrap_or(ptr::null_mut());
            let file = symbol.filename().map(|file| file.display());
            let line = symbol.lineno();

            let loc = file.and_then(|f| line.map(|l| (f, l)));

            // This matches the format generated by folly Symbolizer
            //     @ 0000000000d48b51 namespace::function(int, void*)
            //                        some/file/lol:42
            //                        -> some/inlined/code:44
            // Regex is:
            // (?: {4}@ [[:xdigit:]]{16}| {22} ->) .*
            if i == 0 {
                let _ = writeln!(w, "{:4}@ {:016x} {:#}", "", addr as usize, name);
                if let Some((file, line)) = loc {
                    let _ = writeln!(w, "{:22} {}:{}", "", file, line);
                }
            } else if let Some((file, line)) = loc {
                if INLINED_NAME {
                    let _ = writeln!(w, "{:22} -> {}:{} {:#}", "", file, line, name);
                } else {
                    let _ = writeln!(w, "{:22} -> {}:{}", "", file, line);
                }
            }
        }
    }

    // Make sure everything's flushed before we (maybe) exit
    let _ = w.into_inner();

    match fate {
        Fate::Continue => {}
        Fate::Exit(exit) => {
            #[cfg(unix)]
            unsafe {
                // Use a more brutal shutdown - we don't want any C++ dtors or other cleanup
                // happening when we're crashing out of a fully operable state.
                libc::_exit(exit);
            }
            #[cfg(not(unix))]
            std::process::exit(exit);
        }
        Fate::Abort => {
            #[cfg(unix)]
            unsafe {
                // Remove any existing SIGABRT handler - we don't want redundant
                // diagnostics from this abort.
                libc::signal(libc::SIGABRT, libc::SIG_DFL);
            }
            std::process::abort()
        }
    };
}

/// This funcion should be used to set the hook to be triggered when a panic
/// happens. The [Fate] parameter will define what this handler will do when
/// panicing.
pub fn set_panichandler(fate: Fate) {
    panic::set_hook(Box::new(move |panic| handler(panic, fate)));
}
