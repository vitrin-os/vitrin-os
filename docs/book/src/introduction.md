# Vitrin OS

Vitrin OS is an **agent-first display server**: a small trusted core
(`vitrind`) speaking a capability-native wire protocol, with every legacy
Wayland or X11 application confined to its own per-app nested shim — so that
humans and AI agents can operate the same GUIs concurrently, under granular,
revocable, capability-scoped authorization.

## The sentence the current stack cannot express

> An agent is allowed to fill in one form, in one Firefox window, for the
> next five minutes. It cannot see the password manager open beside it. The
> moment you touch the mouse, you have control back. Hold Escape for a
> second and its authority is gone — mid-click, mid-keystroke, whatever it
> was doing.

Today's agents drive desktops screenshot-by-screenshot: capture, pick pixel
coordinates, click, capture again. That loop is slow and race-prone, and it
runs with all-or-nothing authority — the isolation unit is a whole VM or
desktop session, so one prompt-injected agent's blast radius is everything
on screen.

The underlying protocols cannot express the sentence above. X11 grants every
client near-total authority over the session — that is its model, not a bug.
Wayland achieved isolation by *removing* cross-client capabilities rather
than *mediating* them, and its `wl_seat` singleton has no notion of N
concurrent authenticated principals. AT-SPI2, the accessibility tree agents
use to avoid pixels, is an unauthorized backdoor onto every application's
widgets.

Vitrin is built around the missing primitives instead: **principals** that
authenticate at handshake, **grants** that carry verbs and constraints and
revoke transitively, **consent** rendered by the core that owns the screen,
and **realms** that make scoping structural rather than a policy setting.

## Who this book is for

| You are… | Start at |
|---|---|
| Curious, want to see it work | [Run the demo in five minutes](01-run-the-demo.md) |
| Writing an agent against it | [Your first agent](02-your-first-agent.md) |
| Evaluating the security model | [Grants, consent, and revocation](03-grants-consent-revocation.md) |
| Wondering how apps are isolated | [Realms and shims](04-realms-and-shims.md) |
| Writing a client in another language | [The wire protocol](05-the-wire-protocol.md) |
| Building an alternate core or shim | [Build your own client or shim](06-build-your-own-client.md) |

## Read this before you trust anything here

Phase 1 is complete — every milestone closed on a named integration test
that runs against the shipped binaries with no mock on any seam it claims.
That is a real bar, and it is also a narrow one.

**The sandbox is half-built.** Since P2.6.2 an app in a realm runs in six
namespaces with an identity uid/gid map, zero capabilities and a private mount
table it cannot reshape — verified by the core from outside, and the spawn
refused when it cannot be. Since P2.6.3 it also gets a **Landlock ruleset**
with an enumerated read set, enforced before the shim's `execve`, and a
generated [ABI matrix](isolation-matrix.md) of what that ruleset requires of a
kernel. P2.6.3 is nevertheless **not finished**: that matrix probes nothing, so
it is a table about the build rather than about kernels; the per-kernel one its
criteria ask for does not exist; and the ABI floor that replaced the
degradation ladder narrowed the task rather than finishing it. But there is
since P2.6.4 a **seccomp deny-list** rather than a syscall boundary: it closes
the 13 rows `vitrind --print-seccomp` prints and leaves the rest of the
kernel's syscall surface unenumerated, so the realm is filesystem-confined and
filtered against a named list and not
syscall-confined, and it keeps the invoking user's supplementary groups. Environment hygiene confines
the well-behaved; it does not contain the hostile.

Do not deploy this against untrusted applications or untrusted agents.
[Where this is honest about its limits](limits.md) is the full list, and it
is worth reading before the architecture convinces you of more than it
should.

## Other documents

- [PRD and Technical Architecture](https://github.com/vitrin-os/vitrin-os/blob/main/docs/PRD.md)
  — the canonical vision and design doc.
- [Protocol conventions](https://github.com/vitrin-os/vitrin-os/blob/main/docs/protocol/00-conventions.md)
  — normative wire format, object-id rules, error taxonomy.
- [`protocol/vitrin-v0.xml`](https://github.com/vitrin-os/vitrin-os/blob/main/protocol/vitrin-v0.xml)
  — the IDL, which is the source of truth. Where this book and the IDL
  disagree, **the IDL wins** and this book has a bug.
- [SECURITY.md](https://github.com/vitrin-os/vitrin-os/blob/main/SECURITY.md)
  — what is in scope, and what is known-broken.
