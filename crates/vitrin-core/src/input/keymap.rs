// SPDX-License-Identifier: MPL-2.0
//! The core's own keymap (WS-E.3.1, issue #217; decision **D-028**) — the one
//! place in `vitrind` where a scancode becomes a letter.
//!
//! # Why this exists at all
//!
//! Everywhere else in this crate, `vitrin_shim_seat.key` carries a keysym the
//! core did not compute. Nested mode gets one from winit's already-interpreted
//! `logical_key` ([`crate::input::host_keysym`], issue #118) and headless has
//! no keyboard, so the IDL could say — and did — that no keymap interpretation
//! happens inside the core. libinput says nothing of the kind: it hands a
//! bare-metal backend an evdev scancode and stops, and
//! [`crate::input::invariant_keysym`] maps **no letter, no digit and no
//! punctuation** (its own test asserts `KEY_A → None`). Without this module a
//! session-mode core cannot type. D-028 is the decision that put the state
//! machine here rather than behind a new keymap-relay message.
//!
//! # What is deliberately not reachable from here
//!
//! [`xkb::Keymap::new_from_names`] — RMLVO layout-name resolution,
//! `layout=tr` — **is never called**, and D-028's second sub-question is the
//! whole argument. libxkbcommon's name resolution searches `~/.config/xkb`
//! *before* `/usr/share/X11/xkb` and honours the `XKB_*` environment.
//! Realms run as the core's uid and confinement is Phase 2, so an app inside
//! a realm can write a file the trusted core would then parse. Three things
//! close that, none of them a convention:
//!
//! 1. the context is built `NO_DEFAULT_INCLUDES | NO_ENVIRONMENT_NAMES`, so
//!    libxkbcommon holds **no include path** and reads **no `XKB_*`
//!    variable** — a keymap text that tries to `include` anything fails to
//!    compile rather than reaching a directory;
//! 2. the only constructor takes bytes, and [`CoreKeymap::from_path`] is the
//!    one that touches the filesystem — the core reads the operator's file
//!    itself, at a path it was given, and hands the text to
//!    [`xkb::Keymap::new_from_string`];
//! 3. `crate::input::tests::no_source_file_resolves_a_keymap_by_name` greps
//!    this crate's sources for a call to the RMLVO name constructor and for
//!    any mention of the `XKB_DEFAULT` environment prefix, and it
//!    runs in the **default** build — the one configuration where this module
//!    does not exist and could otherwise rot unwatched.
//!
//! # The two orderings that are easy to get backwards
//!
//! **Resolve before update.** libxkbcommon documents it (`xkb_state_update_key`:
//! "you should prefer to get the keysyms *before* updating the key, such that
//! the keysyms reported for the key event are not affected by the event
//! itself"). Backwards, a modifier applies to its own press — Shift's own
//! press starts reporting `Shift_L` at level 2 — and, worse, a released key
//! resolves at the level its own release just left. [`CoreKeymap::resolve`] is
//! the only place either call appears, so the order is written once;
//! `resolve_runs_before_update_and_the_reverse_order_is_observably_wrong` is
//! the test that fails if it is swapped.
//!
//! **Legacy keysym, then Unicode keysym.** `xkb_state_key_get_one_sym` returns
//! what the keymap says, and a Turkish keymap says `gbreve` — `0x000002bb`,
//! a Latin-3 legacy keysym — for `ğ`. That number is *not* its codepoint
//! (U+011F), and the core reads keysyms as codepoints in exactly one place
//! that matters to a human: [`crate::lock::gate`]'s `printable`, which types
//! the lock-screen passphrase. Its rule is the `keysymdef.h` convention —
//! below `0x100` a keysym is its own codepoint, otherwise it must be
//! `0x0100_0000 | codepoint` — so `ö` (`odiaeresis`, `0x00f6`) types and `ğ`
//! (`0x02bb`) is **silently dropped**, along with `ş`, `ı` and `İ`. Same class
//! of bug in the other direction: the nested backend already emits Unicode
//! keysyms for anything above Latin-1, so without normalisation the two
//! backends would put different numbers on the wire for the same letter.
//! [`unicode_keysym`] settles it: one convention, the one `host_keysym`
//! already uses.

use std::fmt;
use std::path::{Path, PathBuf};

use vitrin_protocol::generated::vitrin_shim_seat::KeyState;
use xkbcommon::xkb;

use super::XKB_KEYCODE_OFFSET;

/// Why a keymap could not be made. Both variants are startup-fatal by
/// D-028(2): a session that comes up with a broken human input path is the
/// trap `deadman::Chord::parse` already refuses to set, one gesture wider.
#[derive(Debug)]
pub(crate) enum KeymapError {
    /// The operator's path could not be read.
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// libxkbcommon refused the text. It reports nothing structured, so
    /// neither does this — the path is what the operator can act on.
    Compile { path: Option<PathBuf> },
    /// The keymap compiles and does not deliver one of the core's own chord
    /// triggers, so the gesture bound to it — possibly the human's off-switch —
    /// would be silently unfirable. Named separately from `Compile` because an
    /// operator whose keymap is *valid but wrong for this purpose* needs to be
    /// told which key, not told their file is broken.
    DisarmsChord {
        trigger: &'static str,
        expected: u32,
        got: Option<u32>,
    },
    /// The file exists and compiles, and somebody other than root or the core
    /// can write it (or a directory above it). Rejected for `realm.toml`'s
    /// reason: whoever can write the keymap chooses what the trusted core
    /// believes the human typed.
    Insecure { path: PathBuf, fault: String },
}

impl fmt::Display for KeymapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "cannot read the keymap at {}: {source}", path.display())
            }
            Self::Compile { path: Some(path) } => {
                write!(f, "the keymap at {} does not compile", path.display())
            }
            Self::Compile { path: None } => write!(f, "the keymap text does not compile"),
            Self::DisarmsChord {
                trigger,
                expected,
                got,
            } => write!(
                f,
                "this keymap does not put `{trigger}` on its own key (expected keysym \
                 {expected:#x}, got {got:?}). A core-owned chord is matched by keysym, so a \
                 layout that moves one leaves that gesture unfirable -- and one of them is \
                 the dead-man switch. The session does not start rather than start with a \
                 missing off-switch",
            ),
            Self::Insecure { path, fault } => write!(
                f,
                "the keymap at {} is {fault}; whoever can write it chooses what the trusted \
                 core believes the human typed -- the rule `realm.toml` is held to, applied \
                 to the file D-028 named instead of the search path it refused",
                path.display()
            ),
        }
    }
}

impl std::error::Error for KeymapError {}

/// The core's compiled keymap and the modifier state it drives.
///
/// One per session, compiled once at startup — not per event and not per
/// realm. A realm's app never sees this keymap: the shim generates its own
/// single-level one and binds each resolved keysym at a plain level (the
/// IDL's normative modifier-suppression rule), which is exactly why
/// interpreting here needs no shim edit and no wire change.
///
/// Not wired to a backend yet, and that is the honest state of #217: there is
/// no DRM backend to feed it (that is #218, WS-E.3.2). This is the decided
/// resolution path plus its tests; the caller arrives with libinput.
#[allow(
    dead_code,
    reason = "WS-E.3.2 (#218) adds the libinput arm that calls it"
)]
pub(crate) struct CoreKeymap {
    /// Held for its lifetime, not for its API: libxkbcommon refcounts these,
    /// and keeping the two owners beside the state makes the "no include
    /// path, no environment" flags a property of the whole object rather than
    /// of one constructor call.
    _context: xkb::Context,
    _keymap: xkb::Keymap,
    state: xkb::State,
}

#[allow(
    dead_code,
    reason = "WS-E.3.2 (#218) adds the libinput arm that calls it"
)]
impl CoreKeymap {
    /// Compile the keymap the operator's path names.
    ///
    /// The core reads the file, then compiles the bytes. That is one syscall
    /// more than handing libxkbcommon the path, and it is the point: nothing
    /// in libxkbcommon's own search machinery is ever consulted, so no file
    /// this call did not explicitly name can be reached.
    /// # The file is audited, and that audit is the whole of D-028(2)
    ///
    /// The decision rejected `new_from_names` because libxkbcommon searches
    /// `~/.config/xkb` before the system tree, so an app running as the core's
    /// uid could plant a keymap the trusted core then parsed. Naming one file
    /// closes the *search*; it does not close the *file*, and without this
    /// audit the same app simply writes the file the operator named. Shipping
    /// the path option without it re-opens, at the named path, exactly the hole
    /// it was chosen to shut.
    ///
    /// So the keymap is held to `realm.toml`'s rule, through the same helper
    /// (`realm::untrusted_writer`): a regular file, owned by root or the core's
    /// uid, not group/other-writable, and the same for every directory above it
    /// — because a writable parent lets an attacker replace the file wholesale.
    /// A symlink is resolved first and the audit applies to the target, since
    /// auditing the link would audit the wrong inode.
    pub fn from_path(path: &Path) -> Result<Self, KeymapError> {
        let resolved = std::fs::canonicalize(path).map_err(|source| KeymapError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::audit(&resolved)?;
        let text = std::fs::read_to_string(&resolved).map_err(|source| KeymapError::Read {
            path: resolved.clone(),
            source,
        })?;
        Self::compile(&text, Some(&resolved))
    }

    /// `realm.toml`'s file rule, applied to the keymap. See [`Self::from_path`].
    fn audit(resolved: &Path) -> Result<(), KeymapError> {
        let euid = rustix::process::geteuid().as_raw();
        let st = rustix::fs::stat(resolved).map_err(|err| KeymapError::Read {
            path: resolved.to_path_buf(),
            source: std::io::Error::from(err),
        })?;
        if rustix::fs::FileType::from_raw_mode(st.st_mode) != rustix::fs::FileType::RegularFile {
            return Err(KeymapError::Insecure {
                path: resolved.to_path_buf(),
                fault: String::from("not a regular file"),
            });
        }
        if let Some(fault) = crate::realm::untrusted_writer(st.st_uid, st.st_mode, euid, false) {
            return Err(KeymapError::Insecure {
                path: resolved.to_path_buf(),
                fault,
            });
        }
        // `skip(1)`: the file itself is already judged above.
        for dir in resolved.ancestors().skip(1) {
            let st = rustix::fs::stat(dir).map_err(|err| KeymapError::Read {
                path: dir.to_path_buf(),
                source: std::io::Error::from(err),
            })?;
            if let Some(fault) = crate::realm::untrusted_writer(st.st_uid, st.st_mode, euid, true) {
                return Err(KeymapError::Insecure {
                    path: dir.to_path_buf(),
                    fault: format!("{fault} (a directory above the keymap)"),
                });
            }
        }
        Ok(())
    }

    /// Compile keymap text the caller already holds. The test entry point,
    /// and the shared body of [`Self::from_path`].
    pub fn from_text(text: &str) -> Result<Self, KeymapError> {
        Self::compile(text, None)
    }

    fn compile(text: &str, path: Option<&Path>) -> Result<Self, KeymapError> {
        // **A NUL byte is refused here, before the FFI.** `Keymap::new_from_string`
        // builds a `CString` internally and panics on an interior NUL — and the
        // panic message contains the whole keymap, so a file with one byte wrong
        // took the session down AND printed its own contents into the log. Both
        // halves are wrong: D-028(2) says an uncompilable keymap must be a
        // refusal rather than a crash, and nothing about a keymap belongs in a
        // panic message. A keymap is text by definition, so this costs nothing
        // a real one needs.
        if text.as_bytes().contains(&0) {
            return Err(KeymapError::Compile {
                path: path.map(Path::to_path_buf),
            });
        }
        // NO_DEFAULT_INCLUDES: no `~/.config/xkb`, no `/usr/share/X11/xkb`,
        // no include path at all. NO_ENVIRONMENT_NAMES: none of the five
        // RMLVO environment overrides, and no `XKB_CONFIG_ROOT`. Together
        // they are what makes "the operator names one file and only that
        // file is parsed" structural.
        let context =
            xkb::Context::new(xkb::CONTEXT_NO_DEFAULT_INCLUDES | xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
        let keymap = xkb::Keymap::new_from_string(
            &context,
            text.to_owned(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or_else(|| KeymapError::Compile {
            path: path.map(Path::to_path_buf),
        })?;
        let state = xkb::State::new(&keymap);
        let mut this = Self {
            _context: context,
            _keymap: keymap,
            state,
        };
        this.verify_core_chords()?;
        Ok(this)
    }

    /// **Refuse a keymap that cannot deliver the core's own chords.**
    ///
    /// The dead-man switch, the attention key, the clipboard gestures, the lock
    /// chord and the screenshot key are all matched by keysym, and under a real
    /// keymap a keysym is what the LAYOUT says it is. A layout that binds
    /// something else to `Escape`'s scancode — or puts the trigger on a
    /// modifier-dependent level, which several `xkb_options` do — leaves that
    /// gesture silently unfirable. For the dead-man chord that is the human's
    /// off-switch, gone, with nothing on screen to say so.
    ///
    /// D-028 named three hazards (pairing, legacy keysyms, ordering) and did
    /// not name this one. It is closed here rather than at the call site
    /// because the call site does not exist yet — #218 wires this — and a check
    /// that has to be remembered by code nobody has written is a check that
    /// will be missing. Constructing a `CoreKeymap` that would disarm the
    /// off-switch is therefore not possible, rather than possible-but-tested.
    ///
    /// Checked at the UNMODIFIED level, because that is where every core chord
    /// trigger lives (`Trigger::VOCABULARY` is Escape, Tab, Enter, Space,
    /// F1-F12, the editing cluster, and the two Supers — no letters, no
    /// digits). A layout that moves one of those has broken more than vitrin.
    fn verify_core_chords(&mut self) -> Result<(), KeymapError> {
        for (name, evdev, expected) in crate::chord::Trigger::VOCABULARY {
            let keycode = xkb::Keycode::new(evdev + XKB_KEYCODE_OFFSET);
            let sym = self.state.key_get_one_sym(keycode);
            let got = (sym != xkb::Keysym::NoSymbol).then(|| unicode_keysym(sym));
            if got != Some(*expected) {
                return Err(KeymapError::DisarmsChord {
                    trigger: name,
                    expected: *expected,
                    got,
                });
            }
        }
        Ok(())
    }

    /// Resolve one evdev scancode to the keysym the app should receive, and
    /// fold the event into the modifier state — **in that order**.
    ///
    /// `None` when the keymap binds nothing to this key. The state is still
    /// updated in that case: a key that types nothing can still be a
    /// modifier's neighbour in a chord, and skipping the update would desync
    /// the state machine from the keyboard.
    pub fn resolve(&mut self, evdev: u32, state: KeyState) -> Option<u32> {
        // The kernel's `KEY_*` domain is xkb's minus 8 (`XKB_KEYCODE_OFFSET`).
        // Getting this wrong is not a crash: evdev 26 (`KEY_LEFTBRACE`) is
        // xkb 34, which on a Turkish keymap is `ğ` — while evdev 34 is
        // `KEY_G`. An off-by-eight types `g` where `ğ` was meant, for every
        // key, silently.
        let keycode = xkb::Keycode::new(evdev.checked_add(XKB_KEYCODE_OFFSET)?);

        // Resolve BEFORE update — see the module docs.
        let sym = self.state.key_get_one_sym(keycode);
        self.state.update_key(
            keycode,
            match state {
                KeyState::Pressed => xkb::KeyDirection::Down,
                KeyState::Released => xkb::KeyDirection::Up,
            },
        );

        (sym != xkb::Keysym::NoSymbol).then(|| unicode_keysym(sym))
    }
}

/// A keysym in the one convention this core puts on the wire: a codepoint
/// below `0x100` is its own keysym, anything else is `0x0100_0000 |
/// codepoint`, and a keysym that types no character is left exactly as the
/// keymap gave it.
///
/// This is the `keysymdef.h` convention [`crate::input::host_keysym`] already
/// follows, applied to the *other* backend so both agree. The three cases,
/// each of which a naive pass-through gets wrong somewhere:
///
/// * **`gbreve` (`0x02bb`) → `0x0100011f`.** A legacy keysym whose number is
///   not its codepoint. Passed through raw it survives to the app (the shim
///   writes the number straight into its dynamic keymap and xkbcommon knows
///   the name), but `crate::lock::gate`'s `printable` — the lock-screen
///   passphrase — reads it as neither a Latin-1 keysym nor a Unicode one and
///   drops it. `ş`, `ı` and `İ` are the same shape; `ö`, `ç` and `ü` are
///   Latin-1 and are not, which is why the failure looks like a typo rather
///   than a bug.
/// * **`odiaeresis` (`0x00f6`) → unchanged.** Its number already *is* its
///   codepoint, and rewriting it to `0x010000f6` would be a second spelling
///   of the same letter on the wire for no gain.
/// * **`Return` (`0xff0d`) → unchanged.** `xkb_keysym_to_utf32` answers `0x0d`
///   for it, and taking that would turn every Enter into a control character
///   the shim would have to guess about. Every keysym whose codepoint is a
///   control character is therefore left alone — which is exactly the set
///   `host_keysym` refuses to encode from the other side.
///
/// Idempotent by construction: a Unicode keysym's codepoint re-encodes to
/// itself.
fn unicode_keysym(sym: xkb::Keysym) -> u32 {
    let raw = sym.raw();
    // **The keypad is left alone**, and this exception is not cosmetic.
    // `keysym_to_utf32` happily maps `KP_0`..`KP_9`, `KP_Add`, `KP_Decimal`
    // and friends to their ASCII codepoints, so re-encoding them threw away
    // which physical key the human pressed: an app could no longer tell `KP_7`
    // from `7`, which spreadsheets, games and remapping tools all care about.
    // The re-encode exists to rescue letters whose LEGACY keysym is not their
    // codepoint (`gbreve` is 0x2bb, not U+011F); the keypad has no such
    // problem, so it needs no rescue and loses information if given one.
    if (0xff80..=0xffbd).contains(&raw) {
        return raw;
    }
    let code = xkb::keysym_to_utf32(sym);
    match char::from_u32(code) {
        Some(ch) if !ch.is_control() && code >= 0x100 => super::UNICODE_KEYSYM_BASE | code,
        Some(ch) if !ch.is_control() => code,
        _ => raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real Turkish keymap, checked in rather than hand-written, because
    /// hand-written keysym constants are wrong the same way the bug is: see
    /// `tests/fixtures/README.md`.
    const TR_KEYMAP: &str = include_str!("../../tests/fixtures/keymap-tr.xkb");
    const US_KEYMAP: &str = include_str!("../../tests/fixtures/keymap-us.xkb");
    /// `us` plus `lv3:caps_switch_latch` — the smallest real keymap on which
    /// resolve-before-update and update-before-resolve disagree. See
    /// `tests/fixtures/README.md` for the table.
    pub(super) const LATCH_KEYMAP: &str =
        include_str!("../../tests/fixtures/keymap-us-lv3latch.xkb");

    /// Evdev scancodes, from `input-event-codes.h`. Named rather than
    /// inlined: every one of them is eight away from the xkb keycode, and a
    /// bare number in a test is how an off-by-eight gets asserted as correct.
    const KEY_A: u32 = 30;
    const KEY_LEFTSHIFT: u32 = 42;
    /// `KEY_LEFTBRACE`. On a Turkish keymap this is `ğ`; evdev **34** is
    /// `KEY_G`, and confusing the two is the failure `resolve`'s comment
    /// names.
    const KEY_LEFTBRACE: u32 = 26;
    /// `KEY_SEMICOLON` — `ş` on a Turkish keymap.
    const KEY_SEMICOLON: u32 = 39;
    /// `KEY_I` — `ı` (dotless i) on a Turkish keymap. `İ` is NOT its
    /// shifted level: Turkish-Q puts the dotted pair on `KEY_APOSTROPHE`,
    /// which is why that constant exists too. Getting this wrong is the
    /// same class of mistake as the scancode offset, and the fixture caught
    /// it.
    const KEY_I: u32 = 23;
    /// `KEY_APOSTROPHE` — `i` on a Turkish keymap, `İ` shifted.
    const KEY_APOSTROPHE: u32 = 40;
    const KEY_ESC: u32 = 1;
    const KEY_ENTER: u32 = 28;
    /// `KEY_CAPSLOCK`. Under `LATCH_KEYMAP` this key chooses level three
    /// while held, so its own press is the event that changes the level it
    /// resolves at.
    pub(super) const KEY_CAPSLOCK: u32 = 58;

    /// `ISO_Level3_Shift` — what Caps resolves to when the sym is taken
    /// **before** the press is folded in.
    pub(super) const LEVEL3_SHIFT: u32 = 0xfe03;
    /// `ISO_Level3_Latch` — what it resolves to once level three is already
    /// on, i.e. what the reverse order reports for the same press.
    pub(super) const LEVEL3_LATCH: u32 = 0xfe04;

    fn press(k: &mut CoreKeymap, evdev: u32) -> Option<u32> {
        k.resolve(evdev, KeyState::Pressed)
    }

    fn release(k: &mut CoreKeymap, evdev: u32) -> Option<u32> {
        k.resolve(evdev, KeyState::Released)
    }

    #[test]
    fn a_keymap_that_does_not_compile_is_an_error_and_not_a_silent_fallback() {
        assert!(CoreKeymap::from_text("this is not a keymap").is_err());
        // The fail-closed half of D-028(2): there is no name-resolution
        // fallback to fall back TO, so a broken file cannot silently become
        // "us".
        assert!(CoreKeymap::from_text("").is_err());
    }

    #[test]
    fn a_keymap_that_tries_to_include_a_file_does_not_compile() {
        // The `NO_DEFAULT_INCLUDES` half, as behaviour rather than as a flag
        // name: `us(basic)` exists on this machine under
        // /usr/share/X11/xkb/symbols, and the context cannot reach it.
        let text = "xkb_keymap {\n\
             xkb_keycodes { include \"evdev\" };\n\
             xkb_types { include \"complete\" };\n\
             xkb_compat { include \"complete\" };\n\
             xkb_symbols { include \"pc+us\" };\n\
             };";
        assert!(CoreKeymap::from_text(text).is_err());
    }

    #[test]
    fn the_real_turkish_keymap_types_the_four_letters_a_codepoint_keysym_would_drop() {
        let mut k = CoreKeymap::from_text(TR_KEYMAP).expect("the tr fixture compiles");

        // g-breve, s-cedilla, dotless-i: the three lowercase letters whose
        // legacy keysym is not their codepoint.
        assert_eq!(press(&mut k, KEY_LEFTBRACE), Some(0x0100_011f)); // ğ
        assert_eq!(release(&mut k, KEY_LEFTBRACE), Some(0x0100_011f));
        assert_eq!(press(&mut k, KEY_SEMICOLON), Some(0x0100_015f)); // ş
        assert_eq!(release(&mut k, KEY_SEMICOLON), Some(0x0100_015f));
        assert_eq!(press(&mut k, KEY_I), Some(0x0100_0131)); // ı
        assert_eq!(release(&mut k, KEY_I), Some(0x0100_0131));

        // İ — the shifted one, which is also the level-arithmetic check.
        assert_eq!(press(&mut k, KEY_LEFTSHIFT), Some(0xffe1));
        assert_eq!(press(&mut k, KEY_APOSTROPHE), Some(0x0100_0130));
        assert_eq!(release(&mut k, KEY_APOSTROPHE), Some(0x0100_0130));
        // And the pair Turkish typists lose to a US keymap: shifted `ı` is
        // plain ASCII `I`, not `İ`.
        assert_eq!(press(&mut k, KEY_I), Some(0x0049));
        assert_eq!(release(&mut k, KEY_I), Some(0x0049));
        assert_eq!(release(&mut k, KEY_LEFTSHIFT), Some(0xffe1));
        // Unshifted, the same key is the dotted `i` — Latin-1, so it needs
        // no rewrite, unlike its capital.
        assert_eq!(press(&mut k, KEY_APOSTROPHE), Some(0x0069));

        // And the contrast that makes the bug look like a typo: ö is
        // Latin-1, so its keysym IS its codepoint and it needs no rewrite.
        // KEY_SEMICOLON's neighbour KEY_APOSTROPHE (40) is `i` on tr; the
        // Latin-1 letter to check is on KEY_LEFTBRACE's row-mate KEY_COMMA.
        let mut k = CoreKeymap::from_text(TR_KEYMAP).expect("the tr fixture compiles");
        assert_eq!(press(&mut k, 51), Some(0x00f6)); // KEY_COMMA -> ö
    }

    #[test]
    fn the_scancode_offset_is_the_one_that_makes_g_breve_and_g_different_keys() {
        let mut k = CoreKeymap::from_text(TR_KEYMAP).expect("the tr fixture compiles");
        // evdev 26 is `ğ` and evdev 34 is `g`. If XKB_KEYCODE_OFFSET were
        // dropped, 34 would resolve where 26 does and this pair would
        // collapse — the exact silent mistyping D-028 names.
        assert_eq!(press(&mut k, 26), Some(0x0100_011f));
        assert_eq!(release(&mut k, 26), Some(0x0100_011f));
        assert_eq!(press(&mut k, 34), Some(0x0067)); // `g`
        assert_eq!(release(&mut k, 34), Some(0x0067));
    }

    #[test]
    fn a_named_key_keeps_its_keysym_rather_than_becoming_a_control_character() {
        let mut k = CoreKeymap::from_text(US_KEYMAP).expect("the us fixture compiles");
        // `xkb_keysym_to_utf32` answers 0x1b for Escape and 0x0d for Return.
        // Taking either would replace the layout-invariant keysym the
        // dead-man chord and the shim's pre-bound table both key on.
        assert_eq!(press(&mut k, KEY_ESC), Some(0xff1b));
        assert_eq!(press(&mut k, KEY_ENTER), Some(0xff0d));
    }

    #[test]
    fn an_unbound_key_yields_nothing_and_still_advances_the_state() {
        let mut k = CoreKeymap::from_text(US_KEYMAP).expect("the us fixture compiles");
        // Well past the fixture's `maximum = 709` xkb keycode. 700 would
        // NOT do: `<I708>` is a declared key bound to `XF86KbdLcdMenu5`,
        // which is exactly the kind of thing a made-up "surely nothing is
        // there" constant gets wrong.
        assert_eq!(press(&mut k, 60_000), None);
        // And the press that follows still resolves — the state machine did
        // not wedge on the unbound key.
        assert_eq!(press(&mut k, KEY_A), Some(0x0061));
    }

    #[test]
    fn an_evdev_code_that_would_overflow_the_offset_yields_nothing() {
        let mut k = CoreKeymap::from_text(US_KEYMAP).expect("the us fixture compiles");
        assert_eq!(press(&mut k, u32::MAX), None);
    }

    /// #217 acceptance criterion (c): the resolve-before-update ordering,
    /// pinned by a case that fails under the reverse order.
    ///
    /// It needed a keymap that can tell the two apart at all. On `us` and on
    /// `tr` every sequence tried gives the same answer either way — which is
    /// exactly why the bug is silent, and why this test is over
    /// `us+lv3:caps_switch_latch`, where Caps chooses level three *while
    /// held* and therefore changes the level its own event resolves at.
    /// Swap the two lines in [`CoreKeymap::resolve`] and both assertions
    /// below invert.
    #[test]
    fn resolve_runs_before_update_and_the_reverse_order_is_observably_wrong() {
        let mut k = CoreKeymap::from_text(LATCH_KEYMAP).expect("the latch fixture compiles");

        // Correct: the press is reported at the level in force BEFORE it,
        // i.e. level one.
        assert_eq!(
            press(&mut k, KEY_CAPSLOCK),
            Some(LEVEL3_SHIFT),
            "the press must not be resolved at the level its own press sets"
        );
        // And the release at the level in force before IT — level three, the
        // key still being held. Under the reverse order these two swap.
        assert_eq!(
            release(&mut k, KEY_CAPSLOCK),
            Some(LEVEL3_LATCH),
            "the release must not be resolved at the level its own release clears"
        );
        assert_ne!(
            LEVEL3_SHIFT, LEVEL3_LATCH,
            "the fixture is only a test if the two levels differ"
        );
    }

    /// The same fixture, read for the *other* thing it proves: a real
    /// keyboard on which one key's press and release resolve to different
    /// keysyms. That is the fact D-028(3) moves pairing off the keysym for,
    /// demonstrated rather than asserted — `crate::input`'s
    /// `a_key_whose_press_and_release_resolve_to_different_syms_...` is the
    /// router half.
    #[test]
    fn a_real_keymap_can_resolve_one_keys_press_and_release_to_different_syms() {
        let mut k = CoreKeymap::from_text(LATCH_KEYMAP).expect("the latch fixture compiles");
        let down = press(&mut k, KEY_CAPSLOCK).expect("Caps is bound");
        let up = release(&mut k, KEY_CAPSLOCK).expect("Caps is bound");
        assert_ne!(down, up);
    }

    /// **A keymap that cannot deliver a core chord is refused at construction**
    /// (WS-E.3.1 review).
    ///
    /// The dead-man switch matches by keysym, and under a real keymap a keysym
    /// is whatever the layout says. A layout that rebinds Escape's scancode
    /// leaves the human's off-switch silently unfirable — so a `CoreKeymap`
    /// carrying one cannot be built at all. #218 wires this; the check lives in
    /// the constructor precisely so #218 cannot forget it.
    #[test]
    fn a_keymap_that_disarms_a_core_chord_cannot_be_constructed() {
        // Rebind Escape's key (xkb 9 = evdev 1) to something else, leaving the
        // rest of a real keymap intact.
        // Rewrite the ESC line by locating it rather than matching exact
        // whitespace, which the fixture's own formatting would otherwise decide.
        let esc_line = US_KEYMAP
            .lines()
            .find(|line| line.contains("<ESC>") && line.contains("Escape"))
            .expect("the fixture binds a keysym to <ESC>");
        // Swap only the KEYSYM on that line, keeping the fixture's own syntax,
        // so the result is a VALID keymap that merely puts something else on
        // Escape's key. Rewriting the whole line produced a keymap that did not
        // compile — and the test then passed for that reason instead.
        let disarmed = US_KEYMAP.replace(esc_line, &esc_line.replace("Escape", "F13"));
        assert_ne!(
            disarmed, US_KEYMAP,
            "fixture check: the ESC line must have been rewritten"
        );
        // **`DisarmsChord`, not just `is_err()`.** A malformed keymap also
        // errors, so `is_err()` passed whether the guard fired or the file
        // merely failed to compile — which is what the mutation run showed:
        // deleting the guard left this green. The variant is the only thing
        // that tells the two apart, and it is why the guard has its own.
        let err = CoreKeymap::from_text(&disarmed)
            .err()
            .expect("a keymap that moves Escape must be refused");
        assert!(
            matches!(err, KeymapError::DisarmsChord { trigger: "esc", .. }),
            "the refusal must be the CHORD guard naming `esc`, not an incidental compile \
             failure: {err}"
        );
        assert!(
            CoreKeymap::from_text(US_KEYMAP).is_ok(),
            "control: the unmodified keymap must still be accepted, or the assertion above \
             passes for the wrong reason"
        );
    }

    /// **The keymap file is held to `realm.toml`'s rule** (WS-E.3.1 review).
    ///
    /// D-028 rejected `new_from_names` because libxkbcommon searches
    /// `~/.config/xkb` first, so an app running as the core's uid could plant a
    /// keymap. Naming one file closes the search and not the file: without this
    /// audit the same app just writes the file the operator named, and the hole
    /// is exactly as open at a different path.
    #[test]
    fn a_group_writable_keymap_is_refused() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("vitrin-keymap-audit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("keymap.xkb");
        std::fs::write(&path, US_KEYMAP).expect("write");

        // Control FIRST: a 0600 file in a sane directory is accepted, so the
        // refusal below cannot pass because the fixture never compiled.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        assert!(
            CoreKeymap::from_path(&path).is_ok(),
            "control: a well-owned keymap must be accepted"
        );

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o664)).expect("chmod");
        let err = CoreKeymap::from_path(&path)
            .err()
            .expect("a group-writable keymap must be refused");
        assert!(
            matches!(err, KeymapError::Insecure { .. }),
            "the refusal must name the reason, not look like a compile failure: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A NUL byte is a refusal, not a panic** (WS-E.3.1 review).
    ///
    /// `Keymap::new_from_string` builds a `CString` and panics on an interior
    /// NUL — and the panic message carries the whole keymap, so one wrong byte
    /// took the session down and printed the file into the log.
    #[test]
    fn a_keymap_containing_a_nul_is_refused_rather_than_panicking() {
        let mut poisoned = String::from(US_KEYMAP);
        poisoned.insert(poisoned.len() / 2, '\0');
        let err = CoreKeymap::from_text(&poisoned)
            .err()
            .expect("a NUL byte must be refused");
        assert!(matches!(err, KeymapError::Compile { .. }), "{err}");
        // The message must not carry the keymap: a panic did, which is how the
        // file ended up in the log.
        assert!(
            err.to_string().len() < 200,
            "the refusal must not quote the keymap back: {}",
            err.to_string().len()
        );
    }

    /// The keypad keeps its identity. `keysym_to_utf32` maps `KP_7` to `'7'`,
    /// so a blanket re-encode would tell an app the human pressed the top-row
    /// digit — information the wire cannot recover.
    #[test]
    fn the_keypad_is_not_flattened_into_ascii() {
        for name in ["KP_0", "KP_7", "KP_Add", "KP_Decimal", "KP_Divide"] {
            let sym = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS);
            assert_eq!(
                unicode_keysym(sym),
                sym.raw(),
                "{name} must travel as itself, not as the ASCII character it types"
            );
        }
        // ...and the rescue the re-encode exists for still happens.
        let gbreve = xkb::keysym_from_name("gbreve", xkb::KEYSYM_NO_FLAGS);
        assert_eq!(
            unicode_keysym(gbreve),
            super::super::UNICODE_KEYSYM_BASE | 0x011f
        );
    }

    #[test]
    fn unicode_keysym_is_idempotent_over_the_three_shapes() {
        let legacy = xkb::keysym_from_name("gbreve", xkb::KEYSYM_NO_FLAGS);
        let once = unicode_keysym(legacy);
        assert_eq!(once, 0x0100_011f);
        assert_eq!(unicode_keysym(xkb::Keysym::new(once)), once);

        let latin1 = xkb::keysym_from_name("odiaeresis", xkb::KEYSYM_NO_FLAGS);
        assert_eq!(unicode_keysym(latin1), 0x00f6);
        assert_eq!(unicode_keysym(xkb::Keysym::new(0x00f6)), 0x00f6);

        let named = xkb::keysym_from_name("Return", xkb::KEYSYM_NO_FLAGS);
        assert_eq!(unicode_keysym(named), 0xff0d);
        assert_eq!(unicode_keysym(xkb::Keysym::new(0xff0d)), 0xff0d);
    }
}
