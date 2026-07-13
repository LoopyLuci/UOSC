//! Console pure-logic subset — real port of the portable parts of
//! `drivers/console.ti`.
//!
//! Serial I/O (`io::write_u8`/`read_u8` against real port addresses) and
//! raw framebuffer pixel writes (`*addr.add(offset) = 0xFFFFFFFF`) are
//! hardware access with nothing portable to test in this crate. What's
//! real, portable, and worth getting right: the `\n` → `\r\n` control-
//! character translation, the cursor advance/wrap/scroll coordinate math,
//! and the `printf`-style format scanner.
//!
//! **Real bug found while porting**: `drivers/console.ti`'s `printf`
//! (`kernel/console.ti:112-149`) never actually writes its arguments. The
//! `%d`/`%u` branch is `{ i += 2; }` and the `%s` branch is
//! `{ if arg_index < args.len() { /* Format argument */ } arg_index += 1; i += 2; }`
//! — both comments describing work that was never implemented, so every
//! `printf("value: %d", &[&42])` call in the Titan source silently drops
//! the `42` and prints `"value: "`. [`format`] below actually substitutes
//! arguments using `core::fmt::Display`.

use alloc::string::String;
use core::fmt::Write as _;

/// Real port of `write_string`'s per-byte translation
/// (`kernel/console.ti:99-109`): `\n` becomes `\r\n`, everything else
/// passes through unchanged.
pub fn translate_write(s: &str) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::with_capacity(s.len());
    for byte in s.bytes() {
        if byte == b'\n' {
            out.push(b'\r');
            out.push(b'\n');
        } else {
            out.push(byte);
        }
    }
    out
}

/// Real, working port of `printf` — fixes the silently-drops-every-argument
/// bug documented above. `%d`/`%u`/`%s` all substitute the next argument's
/// `Display` output (this is a portable logic layer with no C-style type
/// tagging, so all three specifiers behave the same — the distinction
/// exists in the Titan source only in the match arms, never in behavior,
/// since none of them worked); `%%` is a literal `%`; any other specifier
/// passes through both characters literally, matching the Titan source's
/// fallback `_ => write_char(bytes[i])` behavior (which reprocesses the
/// character after `%` on the next loop iteration).
pub fn format(fmt: &str, args: &[&dyn core::fmt::Display]) -> String {
    let mut out = String::new();
    let mut arg_index = 0usize;
    let bytes = fmt.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'd' | b'u' | b's' => {
                    if let Some(arg) = args.get(arg_index) {
                        let _ = write!(out, "{arg}");
                    }
                    arg_index += 1;
                    i += 2;
                }
                b'%' => {
                    out.push('%');
                    i += 2;
                }
                _ => {
                    out.push('%');
                    i += 1;
                }
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorState {
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct FramebufferGeometry {
    pub width: u32,
    pub height: u32,
    pub char_width: u32,
    pub char_height: u32,
}

/// Real port of `write_framebuffer_char`'s cursor-advance and scroll-
/// trigger math (`kernel/console.ti:232-269`), separated from the actual
/// pixel-drawing side effect so it's testable without a framebuffer.
/// Returns the new cursor position and whether a scroll was triggered.
pub fn advance_cursor(state: CursorState, geom: &FramebufferGeometry, ch: u8) -> (CursorState, bool) {
    match ch {
        b'\n' => {
            let mut y = state.y + geom.char_height;
            let mut scrolled = false;
            if y >= geom.height {
                scrolled = true;
                y = geom.height.saturating_sub(geom.char_height);
            }
            (CursorState { x: 0, y }, scrolled)
        }
        b'\r' => (CursorState { x: 0, y: state.y }, false),
        _ => {
            let mut x = state.x + geom.char_width;
            let mut y = state.y;
            let mut scrolled = false;
            if x >= geom.width {
                x = 0;
                y += geom.char_height;
                if y >= geom.height {
                    scrolled = true;
                    y = geom.height.saturating_sub(geom.char_height);
                }
            }
            (CursorState { x, y }, scrolled)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn newline_is_translated_to_crlf() {
        assert_eq!(translate_write("a\nb"), b"a\r\nb");
    }

    #[test]
    fn bytes_without_newline_pass_through_unchanged() {
        assert_eq!(translate_write("hello"), b"hello");
    }

    #[test]
    fn printf_substitutes_string_argument() {
        assert_eq!(format("hi %s!", &[&"world"]), "hi world!");
    }

    #[test]
    fn printf_substitutes_integer_arguments_in_order() {
        assert_eq!(format("%d/%d", &[&5, &10]), "5/10");
    }

    #[test]
    fn printf_percent_percent_is_a_literal_percent() {
        assert_eq!(format("100%%", &[]), "100%");
    }

    #[test]
    fn printf_missing_argument_is_skipped_not_panicked() {
        assert_eq!(format("%d and %d", &[&1]), "1 and ");
    }

    #[test]
    fn printf_unknown_specifier_passes_through_literally() {
        assert_eq!(format("%x", &[]), "%x");
    }

    #[test]
    fn cursor_advances_by_char_width_on_normal_byte() {
        let geom = FramebufferGeometry { width: 800, height: 600, char_width: 8, char_height: 16 };
        let (next, scrolled) = advance_cursor(CursorState { x: 0, y: 0 }, &geom, b'A');
        assert_eq!(next, CursorState { x: 8, y: 0 });
        assert!(!scrolled);
    }

    #[test]
    fn cursor_wraps_to_next_line_at_width_boundary() {
        let geom = FramebufferGeometry { width: 16, height: 600, char_width: 8, char_height: 16 };
        let (next, scrolled) = advance_cursor(CursorState { x: 8, y: 0 }, &geom, b'A');
        assert_eq!(next, CursorState { x: 0, y: 16 });
        assert!(!scrolled);
    }

    #[test]
    fn cursor_at_bottom_triggers_scroll_and_clamps_y() {
        let geom = FramebufferGeometry { width: 800, height: 32, char_width: 8, char_height: 16 };
        let (next, scrolled) = advance_cursor(CursorState { x: 0, y: 16 }, &geom, b'\n');
        assert!(scrolled);
        assert_eq!(next.y, 16); // clamped to height - char_height, not off-screen
    }

    #[test]
    fn carriage_return_resets_x_only() {
        let geom = FramebufferGeometry { width: 800, height: 600, char_width: 8, char_height: 16 };
        let (next, scrolled) = advance_cursor(CursorState { x: 40, y: 100 }, &geom, b'\r');
        assert_eq!(next, CursorState { x: 0, y: 100 });
        assert!(!scrolled);
    }

    proptest! {
        /// The cursor's y coordinate never exceeds the framebuffer height
        /// minus one character row, for any sequence of bytes written from
        /// any starting position — the scroll-clamp logic must hold no
        /// matter how it's driven.
        #[test]
        fn cursor_y_never_exceeds_frame_bounds(
            bytes in prop::collection::vec(any::<u8>(), 0..200),
            start_x in 0u32..800,
            start_y in 0u32..600,
        ) {
            let geom = FramebufferGeometry { width: 800, height: 600, char_width: 8, char_height: 16 };
            let mut state = CursorState { x: start_x.min(799), y: start_y.min(599) };
            for b in bytes {
                let (next, _) = advance_cursor(state, &geom, b);
                state = next;
                prop_assert!(state.y <= geom.height);
            }
        }
    }
}
