// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.
//
// Pure functions extracted from shell_manager for fuzzing.
// This module is shared between the library target (for cargo-fuzz)
// and the binary target (via #[path] include in shell_manager.rs).

/// Quote the program path (argv[0]). `CommandLineToArgvW` uses different
/// rules for the first token: backslashes are literal, and the first
/// unescaped `"` ends argv[0] — there is no way to escape a `"` inside
/// it. So we wrap in plain double quotes and require the input not
/// contain `"`. (Real executable paths never do.)
fn append_wt_commandline_program(cmdline: &mut String, value: &str) {
    debug_assert!(
        !value.contains('"'),
        "executable path cannot contain a literal double quote"
    );
    cmdline.push('"');
    cmdline.push_str(value);
    cmdline.push('"');
}

/// Append a non-first argument, quoting using the `CommandLineToArgvW`
/// convention. Always quotes unconditionally — mirrors
/// `QuoteAndEscapeCommandlineArg` in `src/cascadia/WinRTUtils/inc/WtExeUtils.h`.
/// A `needs_quotes` heuristic is fragile because the OS parser splits on
/// whitespace beyond space/tab (e.g. `\n`, `\r`).
fn append_wt_commandline_arg(cmdline: &mut String, value: &str) {
    cmdline.push('"');
    let mut backslashes = 0;
    for ch in value.chars() {
        match ch {
            '\\' => {
                backslashes += 1;
            }
            '"' => {
                for _ in 0..(backslashes * 2 + 1) {
                    cmdline.push('\\');
                }
                cmdline.push('"');
                backslashes = 0;
            }
            _ => {
                for _ in 0..backslashes {
                    cmdline.push('\\');
                }
                backslashes = 0;
                cmdline.push(ch);
            }
        }
    }
    for _ in 0..(backslashes * 2) {
        cmdline.push('\\');
    }
    cmdline.push('"');
}

/// Build a commandline string from a command and its arguments for WT pane
/// creation. This is the string passed to `create_tab`'s `commandline` param.
///
/// # Security note
///
/// This function is a fuzz target — the quoting must be robust against
/// agent-supplied strings containing shell metacharacters.
pub fn build_wt_commandline(command: &str, args: &[String]) -> String {
    let mut cmdline = String::new();
    append_wt_commandline_program(&mut cmdline, command);
    for arg in args {
        cmdline.push(' ');
        append_wt_commandline_arg(&mut cmdline, arg);
    }
    cmdline
}
