// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The console of the Windows client. The client is a Windows GUI program, so it opens no console window next to the
//! game. When it runs in a console, as `--headless` and `--help` usually do, it attaches to that console so that its
//! output shows there. The Windows API call needs `unsafe`, which this module allows, like the ALSA module.
#![allow(unsafe_code)]

use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

/// Connects standard output and error to the console of the parent process, if it has one.
pub fn attach_parent_console() {
    // SAFETY: AttachConsole takes a process id and has no other preconditions; without a parent console it only
    // returns an error, which leaves the process without a console like before.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
