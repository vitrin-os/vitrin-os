# Build your own client or shim

Three things you might build, in increasing order of effort.

**Licensing, up front, because it decides what you may do:** the protocol,
everything generated from it, the code generator, the conformance
instruments and the SDKs are **Apache-2.0**, patent grant included. You
never have to touch a copyleft file to write a client, an alternate
compositor, or an integration. The MPL-2.0 copyleft binds one group only:
people modifying the trusted core itself.
[`NOTICE`](https://github.com/vitrin-os/vitrin-os/blob/main/NOTICE) is the
normative path→license map.

## An SDK in another language

The codegen already emits Rust and a C header from the same IDL. A third
language is a fork of that path, not a from-scratch effort.

**Start here:**

```
crates/vitrin-scanner/       the generator: IDL XML -> Rust + C header
crates/xtask/                its driver (cargo xtask codegen)
crates/vitrin-golden/        golden frame vectors
sdk/python/                  a complete client, stdlib-only, ~2k lines
```

`crates/vitrin-scanner` is Apache-2.0 precisely so that a third party
writing a Go, TypeScript or C++ SDK forks it.

**The order that works:**

1. **Codec first, against the golden vectors.** Encode and decode every
   message, and check your bytes against `crates/vitrin-golden`. Do not move
   on until they match exactly — every later bug looks like a codec bug
   anyway.
2. **Handshake.** Version and identity hello, resolving to a bound
   principal. Get `version_unsupported` and `auth_failed` right; they are
   the first two errors you will actually hit.
3. **Petition and resolution.** One request minting grant + consent + facets,
   then blocking on `resolved`. Read the effective verbs it returns rather
   than the ones you asked for.
4. **Observe.** Receive a memfd over `SCM_RIGHTS`, honour `stride` (never
   assume `width * 4` — v0 pins it, later versions need not), and map the
   format.
5. **Actuate.** Pointer and text.

**Three things to get right, because they are what a naïve port breaks:**

- **Carry every defined verb, whether or not this core serves it**
  (`observe`, `actuate.pointer`, `actuate.text`, `observe.cursor`,
  `layout.arrange`, `layout.focus`, `designate.file`, `realm.launch`). An
  out-of-range verb bit is fatal `invalid_argument` and kills the connection.
  Omitting one turns a recoverable `unsupported` refusal into a dead socket
  for any user who petitions it. Which of them a deployment *serves* is that
  deployment's business and can change under you — this core refuses
  `observe.cursor` and `designate.file`, and serves the other six — so never
  bake the served set into a client. Transcribe the *values* from the IDL
  rather than assuming consecutive bits — `realm.launch` is 512, because 128
  and 256 are allocated to verbs the IDL does not define yet and are still out
  of range; 64 was one of them until `designate.file` landed on it. The Python
  SDK's `test_verb_parity.py` pins this against the IDL; write the equivalent.
- **Model fatal versus recoverable in your type system.** If your users can
  catch a fatal error and retry, your API is lying to them. The Python SDK
  makes them separate hierarchies for this reason.
- **Read `fd_count` from the header, always.** It is what lets you skip a
  frame you do not understand without desynchronising the descriptor
  stream. A client that infers fd counts from opcodes will corrupt itself
  the first time it meets a message from a newer version.

## An alternate shim

A shim is an unprivileged Wayland (or X11) compositor that forwards frames
up to the core and replays origin-tagged input down into its app. The core
assumes nothing about its behaviour, which is what makes writing a new one
reasonable.

`shim/` is the reference: wlroots, C, Meson, outside the Cargo workspace.
`shim/include/vitrin-protocol.h` is generated from the IDL and is
**Apache-2.0 despite living under `shim/`** — writing a C client must never
require touching copyleft code.

**The contract:**

- Your identity is the socketpair you inherited. There is no handshake and
  no credential — holding the descriptor *is* being that realm's shim.
- Forward composited frames via `vitrin_shim_surface`, as shm or dmabuf. If
  you offer dmabuf, handle `buffer_done(import_failed)` by falling back to
  shm. Do not paint a black frame.
- Receive input on `vitrin_shim_seat` — events only, origin-tagged — and
  replay it into your app through your own `wl_seat`.
- Serve your app whatever protocol surface it needs. That is your problem,
  not the core's, and it is the whole reason this is a separate process.

Text actuation avoids `text-input-v3` deliberately (decision D7) and
synthesises a keymap instead; `shim/docs/` covers why, and it matters if
your shim serves toolkits with their own IME assumptions.

**A patched or replacement shim does not cost you the name.** The trademark
policy draws its line at the trusted core, and `shim/` is outside the TCB by
design.

## An alternate core

The hard one, and the one with real obligations.

`crates/vitrin-core` and `crates/vitrin-ipc` are **MPL-2.0**. The copyleft
is deliberate: the project's claim is a small, auditable trusted core, and a
modified capability kernel should not be shippable as a black box. MPL is
per-file, so this does not reach applications running under it, and MPL
§3.3's Larger Work allowance keeps linking against MIT-licensed wlroots and
Smithay clean.

If your change alters what the core *enforces* — the chokepoint, the grant
lifecycle, the consent surface, the dead-man switch, input origin tagging —
then [`TRADEMARK.md`](https://github.com/vitrin-os/vitrin-os/blob/main/TRADEMARK.md)
asks you to rename or ask first. The reasoning is Firefox/Iceweasel: the
name is what tells someone which build actually enforces the security
claims, and a rename is a remedy rather than a punishment. The default
answer to asking is yes.

**Conformance instruments, all Apache-2.0:**

| Tool | What it checks |
|---|---|
| `crates/vitrin-golden` | Per-pixel + SSIM frame comparison |
| `crates/vitrin-mock-shim` | A controllable synthetic shim peer — **component tests only**, never milestone evidence |
| `fuzz/` | cargo-fuzz targets for protocol decode and `vitrin-ipc` framing, with a checked-in corpus |
| `tests/integration/` | Drives the shipped binary against real apps over a real socket |
| `shim/wlcs/` | Advisory WLCS conformance. **GPL-3.0-only** — never built by default, never linked into `vitrin-shim` |

That last row is the one licensing trap in the tree: `shim/wlcs/` compiles
MPL-2.0 shim sources into a GPL-3.0-only module, which is lawful only
because MPL keeps GPL-3.0 as a Secondary License. **Never add MPL Exhibit B
anywhere in the tree** — it would switch that off and make the module
undistributable.

## Getting it reviewed

The project would rather have a second implementation than a perfect first
one — a protocol with one implementation is a format, not a standard. Open
an issue describing what you are building. Two specific asks:

- **Report IDL ambiguities as bugs.** If you had to guess, the
  `<description>` is underspecified and that is a defect worth fixing while
  v0 is young.
- **Say plainly what you have not tested.** The project's own docs are
  written to that standard, and it is the most useful thing a second
  implementer can offer.
