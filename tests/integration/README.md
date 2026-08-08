# Headless integration suite — CI entry-point contract

Phase-1 integration tests: spawn `vitrind --headless`, drive it with the
Python SDK, assert on the wire and on the flight recorder (plan
`docs/plan/01-phase-1-mvp.md` §2).

**Status: active.** The CI `integration` job (`.github/workflows/ci.yml`)
runs this suite on every PR.

## Why this exists next to `cargo test --workspace`

It is not redundant with the unit tests, and the distinction is the point.
The in-process tests in `crates/vitrin-core/src/session.rs` build a runtime
by calling `start_realm_in` directly — so they never execute `run_session`,
the function that orders startup in the shipped binary.

That leaves one class of regression invisible to them: **startup ordering**.
Issue #77's trap T1 is the example. If a change ever registers the shim
socketpair's event source *after* the fork, the shim blocks on `configure`
forever, every real session wedges permanently and silently, and
`cargo test --workspace` stays entirely green. Only a test that runs
`target/debug/vitrind` catches it.

Everything here therefore drives the **shipped binary** over a real Unix
socket with a real forked realm. Nothing constructs a runtime in-process.

## Layout

**Definition-of-done note (issue #111, plan §5 D12):** only the rows marked
**mock-free milestone gate** below may be cited as evidence that M1.2–M1.5 is
done. Every other test here is a **component test**: it drives the shipped
`vitrind` binary (so it still catches the startup-ordering and wiring bugs
`cargo test --workspace` structurally cannot — see above), but it does so
against `vitrin-mock-shim` on the far seam, which the mock-free gates must
never use as their evidence source. Component tests stay green and keep
their value; they are just never a substitute for the named gate.

| File | Role | Mock-free milestone gate? |
|---|---|---|
| `run.sh` | CI entry point. Bash, exit 0 = pass. | — (harness) |
| `harness.py` | `Core` — boots the binary in a throwaway `XDG_RUNTIME_DIR` (defaults its shim to `vitrin-mock-shim`, `MOCK_SHIM`, unless a real shim path is passed); `IntegrationTest` — per-test deadline and core reaping. | — (harness) |
| `test_runtime_wiring.py` | **Component test.** Issue #77's acceptance criteria (startup-ordering trap T1) against the real core + `vitrin-mock-shim`. Never a milestone gate — it predates and is orthogonal to M1.2–M1.5. | No |
| `test_actuation.py` | **Component test.** P1.8.3 (#42): the SDK's actuation API and typed grant-error exceptions against the real core + `vitrin-mock-shim`. Superseded, for milestone purposes, by `test_real_actuation.py` below. | No |
| `test_multi_realm.py` | **Component test.** WS-E.1.2 (#208): the shipped `vitrind` boots three configured realms and gives each its own shim process, `$XDG_RUNTIME_DIR/vitrin-0/<id>/` tree and realm lock; killing one leaves the other two `Running` with exactly one `realm_died`, and the killed shim is reaped rather than left a zombie (a `SIGCHLD` reaper that stopped at the first realm would leave one); **a grant over the killed realm then refuses `NoSurface` while a survivor's grant still captures** — liveness is judged against the realm a grant names, and judging it against *any* live realm is fail-open across realms; **and the write-side twin: each grant's actuation is delivered into the realm that grant names, hidden or not** (WS-E.1.6/#212 — until it landed the session had one delivery target and every other grant was refused `OperationFailed`; `seat_delivered` names the realm that received each event, so the journal can say *which app* got the keystroke, and here it must name two different ones); a `realm.toml` over the 16-realm cap, without `realm-0`, or with two ids that would fight over one entry in the flat runtime tree (`foo` vs `foo.lock`) is refused at startup naming the numbers, the client-visible consequence, or both colliding ids. Lives here rather than only in `cargo test` for the same reason `test_runtime_wiring.py` does: the multi-realm change put a **loop** inside `run_session`'s startup ordering, and a regression that hoisted it above `install()` would wedge every shim after the first while leaving the in-process tests green. Never a milestone gate — the realms run `vitrin-mock-shim`, and it asserts nothing about which realm the output shows or whose pixels a capture carries; that is `test_two_realms.py`'s (WS-E.1.3, #209). | No |
| `test_demo.py` | The **M1.5 named acceptance gate** (P1.8.7, #110): `examples/agent-demo/run_demo.py`'s headless venue, imported and run against the real chain — `vitrind` → real `vitrin-shim` → real `form-target`, never `vitrin-mock-shim` — with the process spine asserted by ancestry and the clicks/types recorded verbatim at the chokepoint. **The demo is goal-directed and the criterion is a positive content check, not a diff.** The agent is handed a task record it did not author, locates each field by its marker colour in its own capture, clicks, types, confirms ink landed *inside* that field, clicks the located submit button, and then the gate demands that the confirmation frame carry three full-width bands whose colours are exactly what **this task's** 36-bit checksum produces, in order (`examples/agent-demo/README.md` is the normative encoding) — plus the complement, that a *wrong* task's bands do **not** match that same frame — plus the app's own byte-exact `SUBMIT … canon=<hex>` line, the out-of-band ground truth beside the pixels. Every earlier headless threshold ("≥ 400 changed px and a ≥ 64 px dense run", and the 24 px before it) was **deleted, not reworded**: those numbers were derived against `weston-terminal` glyph cells and swapping the app invalidates the derivation. **Two disclosures.** (1) `form-target` is **repo-authored** (`shim/tests/form_target.c`) where `weston-terminal` was third-party; it is a real Wayland client and is neither `vitrin-mock-shim` nor `mock_core.c`, so D12 holds literally, but "the app is written by the same repo that asserts on it" is fair criticism — mitigated by the `click-target` precedent in the M1.4 gate and by the third-party rungs below staying green. (2) The receipt is a **checksum, not glyph recognition**: the agent reads back a function of the record, never the characters. Binary-free classes ride along: `DemoUsesNoMockShim` (grep-proof), `ReceiptEncodingIsPinned` (the Python reference vs. the C and JS restatements), `ReceiptDecodingIsDiscriminating` (wrong/reordered/thin bands rejected), `TaskInputIsValidated`, `DefaultTaskAgreesAcrossLaunchers`, and `ChangeProfileShapeMetrics` — which now pins the **focus-ring trap**: a ring drawn *inside* the field is rejected by the inset rectangle, and the same ring measured *without* the inset is accepted, proving which mitigation does the work. Same C-shim env contract; the nested venue (real Firefox) is workstation-only (`shim/docs/firefox.md`). | **Yes — M1.5** |
| `test_real_app.py` | The **M1.2 exit gate** (P1.9.6, #105): the whole real chain — real `vitrind` → real C shim → real `weston-terminal` — with no mock on any seam. Skips without a built C shim; see the env contract below. | **Yes — M1.2** |
| `test_real_gtk.py` | The GTK rung of the real bring-up ladder (P1.6.6, #106): real `vitrind` → real C shim → real `gtk-entry-probe`, reusing `test_real_app.py`'s real-app mode. Supporting evidence for M1.2's render half, alongside `test_real_firefox.py`. | Supporting — M1.2 |
| `test_real_firefox.py` | The Firefox rung of the real bring-up ladder (P1.6.6, #106): real `vitrind` → real C shim → real pinned Firefox ESR, asserting a real rendered colour and the globals contract, with no mock on any seam. Supporting evidence for M1.2's render half. | Supporting — M1.2 |
| `test_real_capture_fidelity.py` | The **M1.3 exit gate** (P1.8.5, #107): an agent captures a real `solid-client` frame through the real chokepoint; its dominant colour is the served colour, it agrees with the core-internal capture (`vitrind --capture-dump`) by SSIM + per-pixel tolerance via `vitrin-golden-cmp`, and capture-path rate-limit + expiry refuse as `rate_limited`/`expired`. Same C-shim env contract. | **Yes — M1.3** |
| `test_real_actuation.py` | The **M1.4 actuation gate** (P1.8.6, #108): an agent's `grant.pointer` click lands on a real `click-target`'s observed feature (dominant colour flips, D10) and `grant.text` types `héllo→世界` intact into a real `gtk-entry-probe` (D7), each confirmed by the agent's own `observe()` and recorded at the chokepoint. Also carries the **cross-realm actuation guard** (WS-E.1.2 review): two realms, two real `click-target`s, and a grant naming the realm the seat does *not* serve is refused with **no app reporting the click** — proved by `click-target`'s own `HIT` line rather than by the core's account of itself, and bounded by a delivery latency the same run measures. Same C-shim env contract; the GTK rung skips without GTK. M1.4 additionally needs #109, whose **hold-Esc half** is the `test_real_deadman.py` row below and whose **consent half** is the `test_real_consent.py` row below it (#138) — with the scope that gate deliberately does *not* cover recorded under "What the consent gate still does not prove". | **Yes — M1.4 (actuation half)** |
| `test_real_deadman.py` | The **M1.4 dead-man gate** (P1.7.4, #109): a completed hold-Esc chord, applied over a real `click-target` through the real core, revokes a live grant — `observe()` and `grant.pointer.click()` both refuse `Revoked` on the very next check, the real app's target stays unflipped (read from `--capture-dump`, bypassing the now-revoked grant entirely), and the flight recorder journals `dead_man_triggered` then `grant_revoked`. Headless has no physical key to hold, so a `SIGUSR1` to the core (only meaningful on a `dead-man-injector`-feature `vitrind` — see `run.sh`) stands in for the hold; the nested recipe for a *real* held Escape is `shim/docs/firefox.md` §9. Same C-shim env contract as the rest of the real-app ladder. | **Yes — M1.4 (dead-man half)** |
| `test_real_consent.py` | The **M1.4 consent gate** (P1.7.5, #138): a real petition raises a real core-rendered prompt over a real `click-target`; the prompt occludes the human-visible output (the exported footprint is first shown to *be* a card raster at exactly the rectangle the core named — accent ring on all four edges, exact perimeter count, opaque body, buttons, antialiased text — and then to carry zero of the app's target pixels) and never the capture path (the realm-view dump is **byte-identical** to a settled control taken while the app was watched idle, and the agent's own mid-prompt `observe()` still shows the full target and agrees with that dump through `vitrin-golden-cmp`); a mid-prompt actuation on an *already-granted* grant, on a **second connection of the same principal**, refuses `ConsentHeld` **specifically**, the app's own surface stays green in the core-internal dump, and the chokepoint's `consent_held` record falls strictly between the prompt's `shown` and its resolution in the journal; then the identical click lands after the denial (the positive control, last, because `click-target`'s flip is one-way). The decision is resolved by `PetitionRegistry::resolve_human` — asserted as `issuer: "human_consent"` — and the run brands itself `consent_policy: "interactive+consent-injector"`. Headless has no pointer for a human to click with, so an inherited socketpair named by `--consent-injector-fd N` on a `consent-injector`-feature `vitrind` stands in for the click; see "What the consent gate still does not prove" below. Same C-shim env contract as the rest of the real-app ladder. | **Yes — M1.4 (consent half)** |
| `test_real_trust_band.py` | The **trusted-band property gate** (issue #139, refs #85) — mock-free, real-app, and deliberately **not** a milestone gate: plan §5 adjudicated unspoofability out of M1.4's criteria, so this closes a tracked gap on its own terms rather than adding to a closed milestone. A real `click-target`, driven by the agent through the real chokepoint, repaints its **whole surface** — the trusted band's rows included — from black to red, which is the strongest counterfeit available to something that cannot observe the colour it would have to match. Two claims: the band's rows are exactly the app's own colour in **both** capture artifacts (the agent's `observe()` frame and the core-internal `--capture-dump`), before and after, and both of the app's colours carry a channel below the indicator's `[64, 255]` floor so that is proof rather than a 1-in-7-million coincidence; and the core-side witness reports `band_changes == 0` over every composite **it evaluated** (that this is every composite of the session is a fact about `BandWitness::observe` being called from `HeadlessOutput::present`, the backend's single composition path — code, not this gate). The zero is held up by counterweights in the same reply — `probe_changes` increases across the repaint, `composites` rises by **at least two** across a span containing exactly one `band` request (the bound is the point: `answer_band` recomposites before reporting, so a read pays for one composite itself and a bare "it went up" would be satisfied by a witness wired only into the reply path), `tracks_view` refuses a frozen or erased human-visible framebuffer, `band_uniform` refuses a partly-overdrawn or blended band, `refusals == 0`, and `probe_fnv` (a digest of *realm-view* rows just below the band) must equal the digest the harness computes over its own dump of the same instant. Since WS-E.2.3 (#215) the reply also carries `strip_h` and `strip_changes` for the status strip, and this gate asserts **both are zero**: the strip is opt-in precisely so that a ticking clock never enters this byte-for-byte comparison, and a non-zero `strip_h` would mean `tracks_view` is about fewer rows than the gate believes. **The harness never learns the indicator colour**, and the rule the witness holds to is stricter than "export no pixels": every field must be a constant function of the run, independent of the secret's value — `band_witness.rs`'s `a_report_does_not_depend_on_the_bands_colour`, and the same check again over prompt-up composites in `…_with_a_card_up`, pin that mechanically. Same C-shim env contract as the rest of the ladder; rides #138's `consent-injector` channel and feature, so it needs no new CI wiring. **It proves a negative, not that the band is unforgeable to a human's eye** — see below. | No — property gate (#139) |
| `test_two_realms.py` | The **cross-realm capture property gate** (WS-E.1.3, issue #209) — mock-free, real-app, and deliberately **not** a milestone gate: it closes a gap this repo published (`docs/book/src/limits.md`) rather than adding to a closed milestone. Two realms, two real `vitrin-shim` processes, two real `solid-client`s painting **different** colours, and an `observe` grant on each. The claim is that a capture returns the pixels of the realm its **grant** names, never whatever is on the output: the two agents' `observe()` frames differ, each one's dominant colour is its own realm's, each agrees with **its own realm's** `--capture-dump` through the M1.3 gate's own comparator (`vitrin-golden-cmp`, `tol:1,0.001`), and — the assertion a leak actually fails — neither agrees with the *sibling's* dump. `realm-0` sorts first so it is the realm the output is bound to and `second` is hidden, which is the case the leak lived in; the gate asserts nothing about *which* realm a human sees, because that needs an eye and a display (`shim/docs/nested-multi-realm.md`). A second class asserts the hidden realm keeps painting, and its **asymmetry is the whole design**: a **static** `solid-client` in the bound realm and an **animating** `damage-client` — which repaints *only on frame callbacks*, so it is the paced case — in the hidden one. The hidden realm's capture must change across a window, because a realm that stopped receiving `frame_done` would publish one frame and freeze, and its capture would be a stale frame, which `refusal.no_surface` forbids in as many words. The bound realm's capture must **not** change, and that is what makes the first assertion discriminating: were the hidden realm's agent being served the *output's* frame, its capture would be the static one and could not have changed. **Two animating realms would have failed to tell those cases apart** — the symmetric fixture is the vacuous one, and it is rejected on purpose. Same C-shim env contract as the rest of the ladder; needs no new CI feature. | No — property gate (#209) |
| `test_layout.py` | The **layout property gate** (WS-E.1.4, issue #210) — mock-free, real-app, and deliberately **not** a milestone gate, for the same reason `test_two_realms.py` is not. A client holding every verb this core serves calls `focus()` over the wire and moves the output to the realm its grant names; the claim is that the realm which **lost** the output still captures its own pixels. That is the assertion a regression fails: with a binding nobody could move, a core serving "whatever is on the output" and a correct one agree on the bound realm forever, so `test_two_realms.py` could not have caught it. Also asserts the newly bound realm's `observe()` matches its own `--capture-dump` through the M1.3 comparator, and that the two agents are not served one shared frame. `set_fullscreen` gets the honest half CI can assert — served rather than refused, and no corruption — because the two modes are indistinguishable while the output and the realm are the same size, which every headless run is. **Not asserted:** that a human sees the focused realm change; runners have no display (D-019(4)), so that is a manual nested step. | No |
| `test_input_switch.py` | The **input-routing property gate** (WS-E.1.6, issue #212) — mock-free, real-app, and deliberately **not** a milestone gate, for the reason `test_two_realms.py` and `test_layout.py` are not. Two realms, two real `click-target`s under two real C shims, `realm-0` on screen. Two claims that could not both be stated before: an agent's `pointer.click()` under a grant over the **hidden** realm flips *that* app's surface and leaves the visible one untouched (until #212 the core refused it `OperationFailed`, so no app was reached at all), and a **physical** click at the same coordinates flips the realm the output is bound to — one round, two addressing rules, and the journal's `seat_delivered` naming two different realms with two different `origin` tags. A second class holds a physical modifier down across a real `layout.focus` and asserts the losing realm was paid its release (two `key` deliveries to `realm-0`, none to `second`), which is the drain #212's decision 3 exists for. The physical half runs through the **`physical-input-injector`** build (`--physical-input-fd N`), which feeds `input::intake_physical` — the nested backend's own entry point — and never a second, weaker path; CI has no input device and headless is the only backend it runs (D-019(4)), so without it this property has no mock-free gate. **Not asserted:** that a human at real hardware sees or feels any of it. That is `shim/docs/nested-multi-realm.md` step 9, and issue #212 says in as many words that a criterion demanding it in CI would never go green. | No — property gate (#212) |
| `test_attention.py` | The **attention-key property gate** (WS-E.1.7, issue #232) — mock-free, real-app, deliberately not a milestone gate. Two realms, two real `click-target`s under two real C shims. A physical key into the realm on screen makes a real client's `layout.focus` over the *hidden* realm refuse `Preempted` — the loop an in-realm shell is stuck in, since the Enter that sends the request is the physical input that forbids it. One `attention` line on the **`physical-input-injector`** channel then taps the run's configured chord through the same `input::physical_key`, and the identical `focus()` is admitted — **confirmed by `--capture-dump`**, not by the client's account of it: the human's next physical click flips the app in the realm the output moved *to*, read out of that realm's own core-internal dump, while the realm left behind is untouched. The same run asserts the window is single-use (a second layout use, hand still on the keyboard, refuses `Preempted` again and no second `attention_claimed` is journaled) and that a press reaching no layout holder tells nobody and journals `opened: false`. **Not asserted:** that a real Super press on real hardware produces any of it — `SeatInput::physical` is private, headless has no input device, and issue #232 says in as many words that such a criterion could never go green. That is `shim/docs/nested-multi-realm.md` step 10. | No — property gate (#232) |
| `test_real_clipboard.py` | The **cross-realm clipboard property gate** (WS-E.2.1, issue #213) — mock-free, real-app, deliberately not a milestone gate. Two realms, two real `clipboard-peer`s under two real C shims: one owns a `text/plain;charset=utf-8` selection with a known canary string, the other writes whatever selection it is offered into a file. Three `clipboard` lines on the **`physical-input-injector`** channel chord this run's configured gestures through the same `input::physical_key` the nested backend's keyboard handler calls. What is asserted is the **bound**, not "the clipboard works": the offer chord against an empty slot sends nothing and journals `clipboard_refused(offer, empty_slot)`; the promote chord fills the core's slot and **still reaches no other realm** — the sink file stays absent, which is issue #213's "a single gesture transfers nothing" checked against the receiving app rather than against the core's own account; only the second, separate chord in the realm the output moved to produces `offer_selection`, and the receiving app then has the string byte for byte through a real `wl_data_device` transfer. Exactly one `clipboard_promoted` and one `clipboard_offered`, agreeing on one BLAKE3 digest and naming source and sink realms. Finally the flight recorder **and the core's log** are grepped for the literal string and must not contain it. **Substitution stated rather than made quietly:** #213 names alacritty and Firefox, and neither can be made in CI to put a *known* string on its clipboard without a human's mouse, nor to report byte-for-byte what it received; `clipboard-peer` is a real toolkit-free Wayland client a whole Wayland connection past the seam under test, in the same role `click-target` and `solid-client` already fill. **Not asserted:** that a real Ctrl-Shift-Insert on real hardware produces any of it — `SeatInput::physical` is private, headless has no input device, and #213 says in as many words that such a criterion could never go green. That is a nested manual runbook step. | No — property gate (#213) |
| `test_screenshot.py` | The **human-screenshot property gate** (WS-E.2.4, issue #216) — mock-free, real-app, deliberately not a milestone gate. One realm, one real `click-target` under one real C shim. A `screenshot` line on the **`physical-input-injector`** channel presses this run's whole configured chord (`ctrl+print` by default) through the same `input::physical_key` the nested backend's keyboard handler calls, so this gate covers **detection as well as effect** — which is why issue #216's own "CI cannot press a key" criterion is now stale, and this table says so rather than repeating it. What is asserted: one chord leaves exactly **one** file whose name the core minted (`vitrin-<epoch>-NNNN.png`, mode 600, no component a client could influence); its pixels equal the realm view **byte for byte**, decoded with stdlib `zlib` (no image codec enters this repository, in any dependency class) and compared against `--capture-dump`, the core-internal RGBA readback of the same realm — the same ground truth `test_real_capture_fidelity.py` uses, so the comparison is against a different path and not against itself; exactly one `screenshot_written` per chord, and a second chord over the same idle scene digests identically into a different file; and the whole run journals **no `use_decision` and no `grant_minted`**, which is issue #216's title checked rather than asserted. A separate class runs the shipped binary five times to prove `--screenshot-dir` refuses to start — missing path, a regular file, a symlink to a directory, a world-writable directory, a relative path — each naming its reason with a non-zero exit, **plus a control** that a clean private directory passes, without which the five would go green for a binary that refused everything. **Not asserted:** that the trusted band is absent (that is a pixel property, proved in-crate against `TrustedIndicator::for_test()`, where a positive control can locate the secret in the human-visible buffer first), or that a real Ctrl-PrintScreen on real hardware produces any of it — a nested manual runbook step, on the attention key's terms. | No — property gate (#216) |
| `test_launch.py` | The **runtime-launch property gate** (WS-E.1.1, issue #207) — mock-free, real-app, deliberately not a milestone gate. A principal holding `realm.launch` over a `realm.toml` **template** calls `launch()`, receives a realm id it never chose, and a *separate* `observe` petition over that id captures the new app's real pixels; ancestry from procfs shows what forked is the real C shim with a real `weston-terminal` under it. The assertions are the **bounds**, not "launch works": a launch confers nothing over what it launched, a principal without the verb is refused `not_granted` recoverably, a human's `deny` is honoured and the card really did grow the launch field, the grant's rate ceiling refuses `rate_limited` with a nonzero `retry_after_ms`, and the journal's `realm_spawned` names the principal and grant that asked. **Not asserted:** the 16-realm `capacity` refusal (sixteen live shims and apps is a resource claim this ladder should not make for one code — it is proved in-crate) or the *words* on the consent card (the injector reports geometry and pixels, never text). This is the one gate covering a wire-reachable path into `spawn.rs`, so its silent absence would be the costliest of any here. **This row was missing until WS-E.1.5**, which is the same three-places drift issue #229 is about, one place further along. | No — property gate (#207) |
| `test_shell.py` | The **shell-is-a-client property gate** (WS-E.1.5, issue #211) — mock-free, real-app, deliberately not a milestone gate. `examples/shell/run_shell.py` runs as a **separate host-side process** on the other end of a real socket, driven over pipes through its own documented output contract, because the whole claim (PRD §5.1, D-021(4)) is that the switcher is not core code and importing it would quietly weaken that. Two classes. **`RealShellSwitchesRealms`**: the shell holds one `realm.launch` grant per template (#211 decision 4), launches two real `click-target`s into two core-minted realms, and focuses each in turn — with every focus **confirmed by `--capture-dump`**, never by the shell's own account of it: the human's next physical click flips *that* realm's app in that realm's own core-internal dump while the realms left behind stay unflipped. It also asserts that a launch confers nothing, by the second `petition_requested` the shell has to raise over the id the core just minted. **`RealShellDeathAndDenial`**: under `--consent=interactive` with the consent channel answering cards, the shell is `SIGKILL`ed and both realms keep running with the realm it last bound **still receiving the human's physical input** — D-021's own stated cost, asserted as behaviour rather than described; a restarted shell then raises *fresh* petitions (new ids on the `raised` edges, new journal entries), and a **denied** `layout.focus` leaves `focus` showing the core's own `refused(layout.focus, not_granted)` with the bound realm unmoved, because the shell sends the request anyway rather than answering for the core. **Not asserted:** that a human sees any of it (no display on a runner, D-019(4)); that the shell is pleasant, or that a hotkey works — #211 says in as many words that the first is not measurable and the second does not exist. `preempted` handling belongs to `test_attention.py`: a host-side shell's keystrokes never reach `vitrind`, so this gate exercises none of it. That the client has no retry timer is a fact about reading `run_shell.py` and **not** something any assertion here observes — recorded that way because this table is the repo's answer to "what did this run actually prove", and code inspection laundered into it as evidence is the failure the table exists to prevent. | No — property gate (#211) |
| `test_consent_injector.py` | **Component test.** The `consent-injector` channel's fail-closed matrix (no card up, a button the card does not draw, an unknown or spent token, an unparseable line, an over-long line, the peer disappearing) against the real core + `vitrin-mock-shim`. Explicitly **not** a milestone gate: it exists so the gate above stays about the milestone property rather than about the channel's error handling. | No |

### The lock screen has no gate here, deliberately

WS-E.2.2 (issue #214) added a core-drawn lock screen, and this suite has **no
row for it and no file for it**. That is a decision, stated here rather than
left as an apparent omission:

- The lock is **nested-only**. Every `--lock-*` flag is refused at startup with
  `--headless` — a headless session has no physical input device at all, so a
  lock it raised (`--lock-idle` fires on a timer, with no input needed) could
  never be dismissed, and the refusal is what keeps a wedge from being a
  configuration. Headless is the only backend CI runs (D-019(4)).
- The `physical-input-injector` channel, which gives the attention key and the
  clipboard chords their mock-free gates, is headless-only for the same reason
  and therefore cannot reach this one.

A gate added here would have to either prove nothing or weaken that refusal, so
what CI proves instead is the composite (`backend/headless.rs`'s
`the_lock_screen_reaches_human_visible_output_but_never_a_capture` — the lock is
on the human-visible framebuffer and byte-absent from the memfd a capture would
seal), the golden, the gate's pairing behaviour and its recorder entries
(`crates/vitrin-core/src/lock/`), and — the one that matters most — that the
dead-man chord still arms and fires through the real hook stack while the lock
consumes every event. The end-to-end claim is a dated manual runbook,
`shim/docs/nested-lock-screen.md`, which is the split issue #214 asked for in as
many words rather than a criterion that could never go green.

### What the consent gate still does not prove

`test_real_consent.py` closes the gap this section used to describe. Two
things it deliberately does **not** close, stated here rather than left to
be discovered:

- **Unspoofability (issue #85) is not gate-level evidence *here*, and will
  not be.** This gate never learns this session's trusted-indicator colour,
  at all. That secret is never written to any descriptor or file in any
  build, and the pixels the instrumented core exports are exactly the
  consent card's own footprint — which `consent/mod.rs`'s `card_rect` and
  its `the_card_footprint_carries_no_indicator_pixel` test prove is
  indicator-free, because the trusted ring is stroked strictly *outside*
  it and the opaque card is blitted last. **So the gate proves occlusion;
  it does not prove the card is framed in a colour a confined app cannot
  forge.** **Adjudicated 2026-07-25: unspoofability is not an M1.4
  criterion** — neither the milestone table nor its verification list in
  `docs/plan/01-phase-1-mvp.md` §5 names it — **so M1.4 is closed, and the
  gap is tracked separately as #139** rather than folded into a closed
  milestone. Do not cite M1.4 as evidence that the trusted indicator is
  unforgeable.
  (A whole-frame mirror would have let this gate check the band — and would
  have written the session secret to a file a same-uid app can read, which
  is precisely what `consent/indicator.rs` forbids. The smaller claim is
  the honest one.)

  **#139 has since split that gap in two, and closed the half that can be
  closed.** `test_real_trust_band.py` (row above) is a mock-free real-app
  gate for the **negative** half: a confined app's own rendering never
  reaches the band's rows on the human-visible output and never reaches the
  capture path at all. It is *not* a milestone gate and does not reopen
  M1.4. The **positive** half — that a human who learned the colour off the
  band can tell a genuine prompt's frame from a forgery — is
  **permanently component-and-human evidence**: `consent/mod.rs`'s band and
  frame tests, `backend/headless.rs`'s
  `a_prompt_reaches_human_visible_output_but_never_a_capture` and the
  real-app `c_shim_consent_prompt_occludes_…`, `backend/winit.rs`'s
  `no_presentation_path_can_drop_the_trusted_band`, and, for a human,
  `shim/docs/firefox.md` §9's nested recipe. It needs an eye at a real
  display and there is no automatable form of it, so it is recorded as
  closed-to-CI rather than left as an open question.
- **The physical click is not proven here.** The hit test, the 500 ms
  `GUARD_INTERVAL`, the press-arms/release-commits ladder, and the origin
  check that stops an agent answering its own prompt are proven only by
  `crates/vitrin-core/src/consent/grab.rs`'s own tests — which drive the
  private `judge_parts` with real events, including
  `an_agent_cannot_answer_the_prompt_it_petitioned_for` — and by
  `shim/docs/firefox.md` §9 with a human at a mouse. The injector bypasses
  `judge` completely, and the headless router still stacks `NoopHook`, so
  no input of any origin can reach that grab. This is the same shape as
  `test_real_deadman.py`'s SIGUSR1 standing in for a held Escape.

The gate also says nothing about the human-visible frame *outside* the
card footprint (not the scrim, not the ring), and nothing about whether
the card is legible or names the right principal — `consent/render.rs`'s
golden and sourcing tests hold that.

With `click-target` the first of those is **structural, not an omission
an extra assertion could close**, and it is worth knowing which: the
export is clamped to the card's footprint and the card is blitted opaque
last, so those bytes do not depend on the frame beneath them; and the
realm view *outside* that footprint is, measured on the gate's own
artifacts, 93 840 px of one colour (black — the app paints black except
for one centred 160×160 square the card wholly covers). A regression that
erased the realm view from the human-visible output altogether therefore
moves no pixel a human sees in this scenario; the exported window comes
back byte-identical, sha256 and all, which was confirmed by running it.
That defect is caught, at component level, by `backend/headless.rs`'s
`a_prompt_reaches_human_visible_output_but_never_a_capture` (full-bleed
test pattern, bottom-left asserted scrimmed-not-erased) and
`backend/winit.rs`'s `the_nested_window_uploads_the_consent_overlay`.

What the gate *does* now establish about those exported bytes, and did
not before the P1.7.5 repair pass, is their **provenance**: they are
checked to be a raster of vitrind's card at exactly the rectangle the
core named — accent ring on all four edges, its exact perimeter count,
`CARD_BG` over most of the body, both button colours present, and enough
distinct colours to carry antialiased text — *before* the absence of the
app's green is read out of them. An absence over bytes of unproven origin
is satisfied just as well by an empty buffer: an export that never read
the framebuffer at all used to pass this gate, printing its success line
verbatim, and now fails it on the first edge pixel.

`crates/vitrin-core/src/backend/headless.rs`'s
`c_shim_consent_prompt_occludes_the_human_visible_output_but_never_the_real_apps_capture`
remains valuable and remains a **component** test: it is mock-free on the
app seam but builds a `HeadlessView` and a `ShimServer` in-process instead
of driving the shipped binary, which plan §5 D12 disqualifies as milestone
evidence. Cite `test_real_consent.py` for the milestone, that test for the
property in isolation.

Grep-proving the split (run from repo root): every named-gate module boots
its `Core` with an **explicit real shim path** (`shim=str(self.shim_bin)`,
resolved from `VITRIN_C_SHIM_BIN`), never a bare `self.core()` — a bare call
defaults to `harness.MOCK_SHIM`. The `no`/mock mentions those files do
contain are disclaiming prose and assertion strings ("no `vitrin-mock-shim`
in the path"), not the shim they actually run:

```bash
# Every named-gate module passes an explicit shim= path to Core(); none
# relies on Core()'s harness.py default (vitrin-mock-shim). Expect: no output
# (a file listed here would be one that never overrides the mock default).
# test_demo.py is named explicitly: it is the M1.5 gate, and the
# `test_real_*.py` glob does not match it -- which is how this check used to
# skip the one gate file whose mock-freeness it most needed to prove.
rg --files-without-match 'shim=str\(self\.shim_bin\)' \
  tests/integration/test_real_*.py \
  tests/integration/test_demo.py
```

**Mock-freeness is not discriminating power.** Both checks above answer
"what is this test wired to", never "can this test fail". Two gates in this
repo were mock-free and still could not fail on the property they named — the
M1.5 demo gate asked for 24 changed pixels, which the real app's own startup
paint clears; the real-app consent-occlusion proof waited only for the view
to stop being the empty test pattern, which the shim's first commit satisfies
before any client attaches. Before citing a gate as evidence, break the
behaviour it claims to prove and watch it go red.

## Running it locally

```bash
bash tests/integration/run.sh
```

Builds the workspace if `target/debug/vitrind` or
`target/debug/vitrin-mock-shim` are missing, then runs the suite. No
virtualenv, no `pip install`.

## Entry-point contract

The job's steps are gated on this exact path via `hashFiles()`; a guard step
fails the job if other files land in this directory without it, so the gate
cannot silently drift.

- **Entry point:** `tests/integration/run.sh`, invoked by CI as
  `bash tests/integration/run.sh`. Exit `0` = pass, anything else = fail.
- **Budget:** the `run.sh` step is capped at **10 minutes**
  (`timeout-minutes: 10` — the P1.9.1 acceptance criterion). CI runs
  `cargo build --workspace` beforehand as an untimed warm-up step, so
  `run.sh` reuses the already-built binaries rather than budgeting for a
  cold compile.
- **Environment:** GPU-less `ubuntu-latest` runner — pixman rendering + shm
  buffers only (plan §6 D3); nested mode is never a CI dependency (§7 R1).
  Toolchain from `rust-toolchain.toml`; runner `python3` is 3.12 (satisfies
  the SDK's `>=3.11` floor, D8).
- **Python dependencies: none.** Stdlib only, `unittest` rather than pytest,
  SDK imported off `PYTHONPATH`. The job installs no Python packages and the
  SDK is zero-runtime-dependency by design (D8), so this suite needs no
  Python setup step and cannot rot when one drifts. Keep it that way — a
  `pip install` here means editing the workflow too.
- **Native dependencies:** the job installs `libxkbcommon-dev
  libpixman-1-dev`, which `vitrind` links (winit and headless backends).
  For the real-app gate it also runs `shim/ci/install-deps.sh` (Meson +
  wlroots build deps + weston), builds the C shim into `${RUNNER_TEMP}/shim-build`,
  and passes its path as `VITRIN_C_SHIM_BIN`. The M1.3 fidelity gate
  (`test_real_capture_fidelity.py`) needs no *new* CI wiring: its `solid-client`
  app is co-built with the shim by the same `meson compile` (resolved as a
  sibling of `VITRIN_C_SHIM_BIN`, like `gtk-entry-probe`), and its
  `vitrin-golden-cmp` SSIM tool is built by the `cargo build --workspace`
  warm-up that already builds `vitrind`. The M1.4 actuation gate
  (`test_real_actuation.py`) adds no CI wiring either: its `click-target` app is
  co-built with the shim, and it reuses the `gtk-entry-probe` the GTK rung
  already builds. The M1.4 dead-man gate (`test_real_deadman.py`) reuses
  `click-target` too, and needs one extra cargo feature on the `vitrind` this
  job already builds. The M1.4 consent gate (`test_real_consent.py`) reuses
  `click-target` as well and needs a second one, so the list is
  `cargo build --workspace --features
  vitrin-core/dead-man-injector,vitrin-core/consent-injector` (both the "Warm
  build" step and `run.sh`'s own fallback build pass exactly that string, and
  the milestone-gate-drift guard asserts they agree): the SIGUSR1 handler that
  stands in for a completed hold-Esc chord, and the socketpair channel that
  stands in for a human clicking Allow, on a physical-input-free runner.
  Neither feature does anything by itself at runtime — the consent one is
  additionally inert unless the invocation carries `--consent-injector-fd N`.
- **The real-app gate's opt-in knob:** `test_real_app.py` runs only when
  `VITRIN_C_SHIM_BIN` names a built C shim (`shim/build/vitrin-shim`). Unset,
  it **skips** — the local-dev path for anyone without the C toolchain. Set,
  a missing shim or missing `weston-terminal` is a **failure**, not a skip:
  CI sets the variable, so CI can never reach the skip, and a requested gate
  that skipped silently would prove nothing. `VITRIN_SKIP_REAL_APP=1` is the
  explicit local opt-out. Same variable name as the `conformance` job and
  `crates/vitrin-core/src/shim.rs`'s cross-track test. Run it locally with:

  ```bash
  meson setup shim/build shim && meson compile -C shim/build
  bash tests/integration/run.sh   # picks the shim up on its own; see below
  ```
- **A skipped ladder is never silent, and never a pass by accident** (#229).
  The per-module skip above is deliberate, but it used to be undetectable:
  `run.sh` never mentioned `VITRIN_C_SHIM_BIN`, so a run without a built shim
  collected 97 tests, skipped the 25 that make this directory a gate, and
  exited **0** — indistinguishable, in everything a caller inspects, from a
  full mock-free pass. A gate that does not *run* proves exactly as much as
  one that does not *exist*, which is the failure the named-gate lists above
  already exist to prevent. It was not theoretical: #212's
  `test_input_switch.py` was authored, "verified" against this script, and had
  never once executed — two real routing bugs were sitting behind it.

  `run.sh` now: **auto-resolves** `shim/build/vitrin-shim` when the variable is
  unset (nobody who built the shim should also have to remember the knob —
  forgetting it *was* the failure mode); **fails** when the variable is set but
  does not name an executable, because that is a misconfiguration rather than a
  machine state; **announces the mode** before the first test; and **itemises
  every skip** after the last one, on success as much as on failure — the
  failure being guarded against is a success.

  `VITRIN_REQUIRE_REAL_APPS=1` makes a degraded run a hard failure. **CI sets
  it**, so if the shim build step ever silently stops producing a binary, the
  `integration` job goes red instead of green-with-no-evidence.

  What is deliberately *not* enforced is `skipped == 0`. Some skips are honest
  machine states — no GTK dev headers at `meson setup`, no `node` on `PATH` —
  and `test_real_gtk.py` already draws that line ("a loud SKIP, not a fail —
  unlike a missing shim, which is a misconfig"). An absent GTK probe cannot be
  mistaken for a passing GTK gate; an absent shim silently took the whole
  ladder with it.

  One caller-side trap this cannot fix: `bash run.sh | tail` reports **`tail`'s**
  exit status, not the suite's, so the habitual "run it and read the end" makes
  any failure invisible. Redirect and check `$?` instead:

  ```bash
  bash tests/integration/run.sh > /tmp/integ.log 2>&1; echo "EXIT=$?"
  ```
- **The named gates must exist.** `run.sh` carries two lists —
  `MILESTONE_GATES` and `PROPERTY_GATES` — and fails before any test runs if
  one of those modules is absent; CI's "Guard against milestone-gate drift"
  step asserts the same two lists plus the `INJECTORS=` feature line in
  seconds. `unittest discover` cannot tell a gate that was never written from a
  green suite — nothing collected, nothing failed, exit 0 — which is the exact
  shape issue #138 was filed on. Editing the gate table above means editing the
  matching list in the same commit. The split into two lists is not
  bookkeeping: a milestone gate's absence is a claim about that milestone's
  definition of done, and `test_real_trust_band.py` is deliberately not one —
  plan §5 adjudicated its property out of M1.4's criteria, so it is named,
  guarded, and never cited as milestone evidence.
- **Later occupants:** this job also hosts the rest of the M1.5 gates —
  golden frames (P1.9.2), hostile-client tests (P1.9.3) — behind the same
  entry point. The demo gate (P1.8.4/P1.8.7, `test_demo.py`) has landed; it
  adds no new CI wiring, reusing the real-app ladder's `VITRIN_C_SHIM_BIN`
  contract exactly as `test_real_app.py` does. It needs **no distro package
  at all** since the goal-directed rewrite: its app, `form-target`, is
  co-built with the C shim (`shim/meson.build`) rather than installed, so its
  absence beside a built shim is a build misconfiguration the gate **fails**
  on rather than a machine state it skips for. `VITRIN_FORM_TARGET_APP`
  overrides the path, the way `VITRIN_CLICK_TARGET_APP` does for the M1.4
  gate's app.

## Two invariants worth keeping

Both were learned rather than anticipated, and both live in
`harness.IntegrationTest`:

- **Every test has a hard deadline** (`TEST_TIMEOUT_S`). A wedged shim makes
  `observe()` block forever; without the deadline the suite would hang until
  CI's 10-minute cap and report a nameless timeout — the worst possible
  reporting for the exact bug this suite exists to catch. With it, a wedge
  fails as a named test whose message points at trap T1.
- **Every core is reaped, pass or fail.** A test that failed between
  spawning a core and cleaning it up used to orphan a `vitrind` and its
  shim, which kept composing while later tests ran.

Related: `ANIMATE_FRAMES` in `harness.py` is a **CPU budget, not a
duration** — headless has no output clock, so a paced shim composes as fast
as the runtime loop dispatches it, and the frame count is the only thing
bounding how long it spins.
