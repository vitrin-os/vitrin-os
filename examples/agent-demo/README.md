# Vitrin OS demo agent

`run_demo.py` is Phase 1's integrating demo agent — and, in its headless
venue, the **M1.5 acceptance test**. It is *goal-directed*: the agent is handed
a **task record it did not author** — field names and values — and it fills that
record into a form in a real app inside `realm-0`, submits it, and then proves
**from pixels alone** that the confirmation reflects exactly the values it was
told to enter.

> connect (the static demo identity) → request the one MVP grant (observe +
> `actuate.pointer` + `actuate.text` on `realm-0`, `while-running`) → await
> consent → **for each field**: locate it by its marker colour in the agent's
> own capture, click its centroid, type the value, confirm ink landed inside
> that field → locate and click the submit button → decode the confirmation's
> three receipt bands and compare them against bands computed from the
> **supplied** task at runtime.

The same agent code drives both venues. Only *which real app stands behind the
shim* differs, plus one nested-only preamble (navigating Firefox to the served
page).

## What the demo does and does not claim

Read these three sentences before reading anything else, because every earlier
version of this document over-claimed at least one of them.

- **There is no language model here.** The agent is deterministic. It "locates"
  a field by scanning its own captured frame for a known marker colour and
  clicking that region's centroid. Nothing reasons, plans, or interprets.
- **The receipt is a checksum, not glyph recognition.** The agent never reads
  back the characters it typed. It reads back a **36-bit function of the record
  the app received** and checks that it equals the same function of the task the
  agent was given. A sentence saying the agent "read back what it typed" would
  be false.
- **The task is an input, not a constant.** `--task K=V` is repeatable and
  order-preserving, and the expected bands are computed from whatever was
  supplied, at runtime. That is what makes the assertion non-vacuous: it cannot
  be a hardcoded constant that would pass regardless.

## The receipt encoding (normative)

This section is the definition. `run_demo.py`'s Python is the **reference
implementation**; `examples/agent-demo/form.html` (JavaScript) and
`shim/tests/form_target.c` (C) restate it, and
`tests/integration/test_demo.py` pins both against the reference on the
shipped default task. Where any of the three disagrees with this section, this
section and the Python win, and the other implementation is the bug.

**The task** is an ordered sequence of `(key, value)` pairs. Order is part of
the record: reordering the same pairs is a *different* record and produces
different bands.

**The canonical string** is

```
canon = "\n".join(f"{k}={v}" for k, v in task)
```

— no trailing newline, no escaping, UTF-8 when hashed.

**FNV-1a, 32-bit** (six lines in any language, no library, no ambiguity):

```
h = 0x811c9dc5
for each byte b of input:
    h = ((h XOR b) * 0x01000193) mod 2**32
```

**Band `i`** (for `i` in `0, 1, 2`), where `str(i)` is the decimal digit:

```
h = fnv1a32(utf8(canon + "#" + str(i)))
r = ((h >> 8) & 0xF) * 0x11
g = ((h >> 4) & 0xF) * 0x11
b = ( h       & 0xF) * 0x11
```

**The confirmation view** paints three full-width horizontal bands, band `0`
topmost, in the region below the echo strip: `BAND_TOP = 96` rows down in the
headless venue's pinned 640×480 realm view (so three 128-row bands), and the
CSS equivalent in `form.html`.

Two properties of the encoding are load-bearing and worth stating explicitly:

- **Every channel is a multiple of `0x11`.** That is this repository's
  established convention for a colour that survives the capture path *and* a
  4-bit-per-channel histogram **exactly, with no tolerance** — the same reason
  `shim/tests/click_target.c` picks its three colours that way and the same
  quantisation `tests/integration/harness.py`'s `dominant_colour` /
  `locate_colour` apply. So the band check is an equality, never a distance.
- **Three bands are 36 bits.** A *wrong* record whose three bands all matched
  would be a coincidence with probability ≈ 2⁻³⁶ ≈ 1.5 × 10⁻¹¹. That is the
  entire strength of the pixel claim, and it is a checksum's strength — not
  evidence about individual characters.

For the shipped default task
(`name=Ada Lovelace`, `email=ada@example.org`) the canonical string is
`"name=Ada Lovelace\nemail=ada@example.org"` and the bands are
`#993300`, `#aacc33`, `#cc5566`.

## Task input rules

`--task K=V` may be given more than once; the pairs keep their order. Given
none, the shipped default is used. Both venues' forms have exactly **two**
fields, so exactly two pairs are required.

Rejected at parse time, before anything connects:

- a value containing **any C0 (U+0000–U+001F), DEL (U+007F) or C1
  (U+0080–U+009F) character**. The IDL makes every one of those except
  `\n` and `\t` a **fatal** `invalid_argument` on
  `vitrin_actuator_text.type` (`protocol/vitrin-v0.xml`), and `\n`/`\t` are
  rejected here too: they are *actuations* (Return, Tab) rather than
  characters a field would hold, so a record containing one could never round
  trip through the form.
- a value whose UTF-8 exceeds **4096 bytes**, the IDL's cap on one `type`
  payload.

Keys are validated the same way. A key is never typed — it reaches the app as
an argv `--field NAME` and the page as a `?k=` query parameter — so this is
hygiene rather than protocol conformance, and it is stated as such.

## Running it

The launcher is a Rust `xtask` that writes a throwaway one-principal registry
(`principals.toml`, mode 0600) and realm config, starts the shipped `vitrind`,
waits for its socket, runs this script against it, then tears the core down
with SIGTERM and prints the flight-recorder path. Trailing `--task K=V`
arguments are forwarded to the agent verbatim.

```console
# Headless venue — no display, no browser, no GPU (this is the CI gate):
cargo xtask demo --headless

# ... with a task the agent has never seen:
cargo xtask demo --headless --task name=Grace --task email=grace@example.net

# Nested venue — needs a display and Firefox ESR:
cargo xtask demo
```

### Headless (`cargo xtask demo --headless`)

The app is **`form-target`** (`shim/tests/form_target.c`), a bare
`wl_shm` + `xdg-shell` + `wl_pointer` + `wl_keyboard` client co-built with the
shim. It paints, in surface-local pixels at the pinned 640×480 realm view:

| Feature | Colour | Rect |
|---|---|---|
| paper | `#ffffff` | whole view |
| field 0 | `#00ff00` | (40, 96)–(600, 140) |
| field 1 | `#00ffff` | (40, 176)–(600, 220) |
| submit | `#ffff00` | (40, 256)–(600, 312) |

with 4 px black borders drawn *outside* each rectangle, so the rectangle the
agent locates by colour is exactly the marker's own extent. A click inside a
field focuses it; every key the client resolves through the shim's **dynamically
generated** keymap (with xkbcommon, exactly as GTK/Qt/Firefox do — decision D7)
appends its UTF-8 to the focused field; a click inside the submit button
repaints the whole surface as the echo strip plus the three bands.

It also prints one byte-exact line to stdout:

```
SUBMIT fields=2 canon=<hex> f0=<hex> f1=<hex> band0=rrggbb band1=rrggbb band2=rrggbb
```

That is the **out-of-band ground truth** beside the pixels — the role
`ENTRY_HEX` plays for the D7 text gate in `test_real_actuation.py`. Hex, not
text, because a mangled character and a correct one can render identically.

`form-target` rasterises **no font**. Typed bytes are drawn as one filled 4×12
ink cell per received UTF-8 byte. That is enough for "ink landed inside the
field I clicked" and nothing more; the demo's proof of *content* is the receipt
checksum and the `SUBMIT` line, never glyph recognition.

`form-target --bands CANON` computes and prints the three band colours and
exits, touching no Wayland at all. That is how the gate pins the C
implementation against the Python reference on a runner with no compositor.

### Nested (`cargo xtask demo`)

A real Firefox ESR runs in `realm-0`. The agent serves `form.html` from a
stdlib `http.server` bound to `127.0.0.1:0` on a daemon thread, types that
local URL into Firefox's URL bar, waits for the page's green field marker to
appear, and then runs the *identical* field loop and receipt decode.

`form.html` uses the same colours and the same reading order as `form-target`,
so the agent's locator code is literally the same code in both venues. On
submit it hides the form, shows the echo strip plus the three bands, and fires
an out-of-band beacon:

```js
new Image().src = "/submitted?" + urlencoded ordered pairs;
```

A `GET` on purpose, not a form `POST`: a navigation would replace the very
receipt the agent is about to read. `_LocalPage.submitted` records those
ordered pairs — the nested venue's byte-exact ground truth, the analogue of
`form-target`'s `SUBMIT` line.

There is deliberately **no Enter handler** in either venue. The agent clicks the
button it located, so per-field typing carries no trailing newline and
"the form was submitted" is itself a pointer-actuation proof.

Firefox's path defaults to `/usr/bin/firefox-esr`; override with
`VITRIN_DEMO_FIREFOX=/path/to/firefox`. The URL-bar click is a **pinned
geometry constant** (`(640, 72)` at the pinned 1280×800 nested window), not a
vision model — version 1 has no semantic tree. Override it with
`VITRIN_DEMO_URL_BAR=x,y` when a different Firefox build lays its toolbar out
elsewhere; if the page never loads, the failure says exactly that, and names
this variable, rather than failing later with a confusing "no green field
found".

## The focus-ring trap

Worth naming, because this repository has already been burned twice by defects
of exactly this shape (see `docs/plan/01-phase-1-mvp.md`'s D12 seam table).

The per-field check is "ink landed **inside** the field I clicked". A real app
draws a **focus indicator** when a field is clicked — and that indicator is a
change *inside* the field's bounding box that no typing produced. A naive
"did anything change in the field?" check is satisfied by it with nothing
typed at all. `form-target` draws its focus ring 2 px *inside* the field
rectangle on purpose, so the headless gate actually springs the trap rather
than only asserting about it.

Two mitigations, both applied:

1. The per-field ink profile is baselined **after the click and before the
   type**, so the focus indicator is inside the baseline rather than inside the
   measured diff. The baseline is taken after the in-rect diff between
   consecutive captures has gone quiet, so it cannot race the indicator's
   arrival.
2. The measured rectangle is **inset** past the ring
   (`FIELD_RECT_INSET`), so ring pixels are excluded geometrically even if
   mitigation 1 somehow lost the race.

`ChangeProfileShapeMetrics` in `tests/integration/test_demo.py` pins the
distinction on frames assembled in-process: a ring-only diff is rejected, the
same diff measured *without* the inset is accepted (proving the inset is what
does the work), and typed ink is accepted.

## It doubles as the M1.5 acceptance test

`tests/integration/test_demo.py` imports this script's `run()` entry point and
drives the **headless** flow against a real `vitrind` (via `harness.Core`).
The acceptance criterion is no longer "pixels moved" — a diff, satisfiable by
incidental repaint — but a **positive content check** plus an out-of-band
byte-exact one:

1. the after-frame contains three full-width solid bands whose colours are
   exactly what this task's checksum produces, in order;
2. a *wrong* task's expected bands do **not** match that same frame;
3. `form-target`'s own `SUBMIT ... canon=<hex>` line equals the hex of the
   agent's canonical string, byte for byte;
4. the flight recorder reconstructs the session in order — handshake/bind →
   petition → resolution → captures carrying frame digests → an allowed `move`
   at each clicked centroid → an allowed `type` per field whose `chars` equals
   that field's value length.

Because the integration suite's entry point is
`unittest discover -p 'test_*.py'` (`tests/integration/run.sh`), this gate
rides CI with no workflow edit.

### Disclosure: the M1.5 gate's app is now repo-authored

State this plainly, because it will be raised and it should be.

`form-target` is a real Wayland client — it binds real globals, commits real
`wl_shm` buffers, and resolves real keys through the shim's real dynamically
generated keymap. It is **neither `vitrin-mock-shim` nor
`shim/tests/mock_core.c`**, so D12 (`docs/plan/01-phase-1-mvp.md` §5) holds
literally: no mock sits on any seam this milestone claims. The previous
headless app was `weston-terminal`, a third-party program.

But *"the app is written by the same repository that asserts on it"* is a fair
criticism of this change, and it is a real reduction in independence. The
mitigations, named in the same breath rather than buried:

- **Precedent, not novelty.** The M1.4 actuation gate (#108) has used a
  repo-authored app, `click-target`, since it landed, for the same reason: no
  third-party app gives a GPU-free, unambiguous, whole-frame response to a
  specific input that an `observe()` can assert without a human eyeballing it.
- **The third-party rungs stay green and stay in CI.**
  `tests/integration/test_real_app.py` (weston-terminal),
  `test_real_gtk.py` (a GTK app) and `test_real_firefox.py` (real Firefox)
  continue to exercise the same shim, the same transport and the same
  actuation chokepoint against software nobody here wrote. A regression that
  only `form-target` tolerates would show up there.
- **The pixel claim has an independent witness.** `form-target`'s `SUBMIT`
  line is produced by the app, read by the gate, and compared against a value
  the *agent* computed. A repo-authored app that painted the right bands
  without receiving the right bytes would have to lie in both places
  consistently.

What this does **not** rule out: a `form-target` bug that happens to make both
the bands and the `SUBMIT` line agree with a record the agent never delivered.
Only a third-party app can close that, and no third-party app offers this
response shape. Read the gate as what it is.

## Notes

- Pure stdlib, Python ≥ 3.11, zero runtime dependencies — the SDK's posture
  (decision D8). Frames are written with `Frame.to_png`; the local page is
  served with `http.server`. No Pillow, no requests.
- `form.html` is a **data file**, resolved as
  `pathlib.Path(__file__).parent / "form.html"`. It is not a Python string
  literal, because 60 lines of HTML + JS is where the old `_SOLID_PAGE`
  idiom stops paying.
- On any assertion failure the script saves the before/after frames as PNGs
  under `--out` and prints the flight-recorder path, so a failure is
  diagnosable from the artifacts alone.
