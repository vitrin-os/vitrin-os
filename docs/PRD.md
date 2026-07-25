# Agent-First Display Server — PRD and Technical Architecture

**Author:** Muhammed Taha Ayan · **Date:** 12 July 2026 (rev 1.4 — consent ladder + app provenance; rev 1.3 — network authority + SSH principals; rev 1.2 — filesystem authority + credential wallet; rev 1.1 — tiered scope; original 11 July 2026) · **Status:** Vision proposal (Phase 0)
**Project name:** **Vitrin OS** — daemon binary `vitrind`; GitHub org `vitrin-os`; npm scope `@vitrin-os`; crates.io `vitrin-os` / `vitrind` (namespaces claimed 12 July 2026; see Naming section for the naming history)

---

# DOCUMENT 1 — PRODUCT REQUIREMENTS DOCUMENT (PRD)

## TL;DR

- We propose an open-source, **agent-first display server**: one trusted, capability-native core speaking a new principal-facing protocol, with every legacy X11/Wayland app confined to its own isolated per-app nested shim server — so that humans and AI agents can concurrently observe and operate GUIs under granular, token-scoped, revocable authorization.
- The incumbent stack for agent GUI control (Anthropic's reference: [Xvfb](https://www.x.org/archive/X11R7.6/doc/man/man1/Xvfb.1.xhtml) + a desktop + screenshots + [xdotool](https://github.com/jordansissel/xdotool); [AWS WorkSpaces for AI agents](https://aws.amazon.com/workspaces/ai-agents/); Microsoft [Windows 365 for Agents](https://www.microsoft.com/en-us/windows-365/agents)) is coarse-grained, screenshot-and-pixel based, and either proprietary or one-VM-per-agent; none provides per-surface, per-principal, capability-scoped authority or a race-free observe-act model.
- This is a manifesto + protocol spec + reference-implementation plan. Scope is tiered (§5): permanent security invariants; a deliberately narrow v1 (headless fleets + local nested operation); and an explicitly _claimed_ horizon — session mode, a mission-control shell, native toolkit backends, a capability-remoting protocol. Success is measured in spec adoption, reference-implementation milestones, and community, not revenue.
- Two further pillars complete the security story: **designation-is-authorization filesystem authority** (every file reaches an app as a consented, kernel-enforced fd; ambient filesystem access absent by construction — the mechanism that bounds ransomware to a realm) and a **display-server-level credential wallet** (OID4VC/OID4VP presentations as grants, with consent that can require physically-originated input; apps and agents receive short-lived scoped tokens, never credentials). A third, **network authority**, extends the same discipline to sockets: a realm has no ambient network reachability (own loopback-only network namespace), and egress is a designated, host:port-scoped grant — closing the reachable-service escape class (`ssh localhost`, host-spawn helpers, container sockets, D-Bus activation) by construction. Standing grants for gesture-less software ride a consent-persistence ladder whose durable rungs are gated on **verified app provenance** — Sigstore-style identity-bound signing plus a transparency log, deliberately not a blockchain.

## 1. Problem statement

### 1.1 X11's ambient authority

The X11 protocol grants every connected client near-total authority over the session: any client can enumerate all windows, read any window's contents, synthesize input into any other window, and install global keyboard grabs. Under X11 any application can keylog any other application and screenshot any window. This is not a bug to be patched; it is the protocol's model.

### 1.2 Wayland's consent model without completeness

Wayland fixed ambient authority by isolating clients — a client cannot see other clients' surfaces or input by default. But isolation was achieved by _removing_ capabilities rather than _mediating_ them, so the ecosystem has spent a decade re-adding cross-client features (screen capture, input synthesis, global shortcuts, remote desktop, accessibility) one protocol at a time, unevenly across compositors. The building blocks now exist but are scattered and optional:

- **wp_security_context_v1** (Wayland staging protocol; merged into [Flatpak](https://flatpak.org/) with the private-socket work in the 1.16 release) lets a sandbox engine tag a connection with a reverse-DNS sandbox-engine name (e.g. `org.flatpak`), an app ID, and an instance ID, so the compositor can identify and restrict sandboxed clients. This is the closest existing analog to connection-time identity, but it carries no per-object rights — and it had a known regression that led Flatpak maintainers to debate turning it off by default in 1.16.x (issue #6019).
- **ext-transient-seat-v1** creates short-lived virtual seats (built for wayvnc/remote desktop) — proof multi-seat is possible, but the `wl_seat` singleton remains the default mental model.
- **libei / libeis (EIS)** (Peter Hutterer; libei 1.0 froze the C API and protocol) provides emulated input that is _distinguishable inside the compositor_ from physical input — yet, per the EI protocol docs, "to Wayland clients … emulated input events are indistinguishable from real input devices." This is exactly the primitive an agent-input pipeline needs, but it lives beside, not inside, an authorization model.
- **xdg-desktop-portal** RemoteDesktop/ScreenCast **restore tokens** provide persistent, user-approved grants — an OAuth-refresh-token-shaped idea already in production, but scoped to portals, not to a general object model.

The seat singleton is the deepest flaw for our use case: Wayland's `wl_seat` models "the cursor as master of the UI." One focus, one pointer, one keyboard. There is no first-class notion of N concurrent authenticated principals, so a human and an agent (or two agents) cannot cleanly co-inhabit a session with independent, preemptible input. Notably, multi-pointer X (MPX/XInput2, Peter Hutterer, merged in X Server 1.7 in 2009) proved multi-principal input was technically achievable seventeen years ago; Wayland regressed to the singleton for isolation reasons.

### 1.3 The accessibility backdoor (AT-SPI2)

Agents that avoid pixels use the accessibility tree. On Linux that is [AT-SPI2](https://gitlab.gnome.org/GNOME/at-spi2-core) over D-Bus, and it is an unguarded backdoor: any process on the session bus can read the entire widget tree of every application and invoke actions (press buttons, set text) with zero consent. It is simultaneously too open (no authorization) and, on Wayland, too broken — the pull-based, per-application-tree, high-round-trip design performs poorly and was largely unmaintained after the Sun/Oracle desktop team was shuttered. GNOME's **[Newton](https://lwn.net/Articles/971541/)** project (Matt Campbell, funded by the [Sovereign Tech Fund](https://www.sovereign.tech/) via the GNOME Foundation) is a Wayland-native rewrite: push-based, per-surface trees, coordinates relative to the surface, built on the **[AccessKit](https://accesskit.dev/)** schema (itself derived from [Chromium](https://chromium.googlesource.com/chromium/src/+/HEAD/docs/accessibility/overview.md)'s accessibility tree), designed so Flatpak-sandboxed apps expose trees without an AT-SPI sandbox hole. Newton validates our semantic-tree design — but it is not built for authorization, multi-principal arbitration, or agents, and per LWN's OSSNA 2024 coverage, "the protocols for Newton are not yet finalized," with only "prototype implementations for AccessKit, Orca, and GNOME's Mutter display server."

### 1.4 Prompt-injection blast radius

The security motivation is concrete. Simon Willison's **lethal trifecta** (16 June 2025): an agent that combines "Access to your private data … Exposure to untrusted content … [and] the ability to externally communicate in a way that could be used to steal your data" can be turned against its user — "an attacker can easily trick it into accessing your private data and sending it to that attacker." A computer-use agent driving a full desktop has all three by construction: it sees everything on screen (private data), reads web/document content (untrusted), and can operate any app including mail and terminals (exfiltration). Today's isolation unit is the whole VM/desktop, so a single prompt-injected agent's blast radius is the entire session. There is no structural way to say "this agent may operate only this one form in this one app, may not read the password-manager window beside it, and loses all input the instant a human touches the keyboard."

### 1.5 Coarse, proprietary cloud offerings

The market already recognizes the problem and is answering it coarsely:

- **AWS WorkSpaces for AI agents** — moved from preview to **GA around 30 June / 1 July 2026**. Agents get their own managed Windows desktop, authenticate via **IAM**, connect via a managed **[MCP](https://modelcontextprotocol.io/)** endpoint, are audited through **CloudTrail/CloudWatch** with screenshots stored in S3, and can be supervised via three modes stated verbatim in AWS docs: "VIEW_ONLY allows users to observe agent actions … VIEW_STOP allows users to observe and click a stop button to immediately remove the agent's session access. DISABLED runs agents without user visibility." Isolation unit: a whole WorkSpace.
- **Microsoft Windows 365 for Agents** + **Microsoft Execution Containers (MXC)** (Build 2026, 2 June 2026). Per the Windows Developer Blog: "Windows 365 for Agents, now generally available … the agent runs in an Intune-managed Cloud PC … If compromised, impact is contained to a disposable cloud instance," alongside the MXC SDK, with each agent assigned "a local ID or a cloud provisioned identity backed by Entra." Isolation unit: a session/Cloud PC.

Both are the right instinct — identity per agent, audit, human oversight — executed at VM/session granularity and locked to a proprietary OS and cloud. There is no open, fine-grained, per-surface, capability-scoped counterpart. That is the gap.

### 1.6 The incumbent being obsoleted

Anthropic's own computer-use reference implementation is still Ubuntu + Xvfb + a desktop + screenshots + `xdotool` input inside a Docker container. The loop is: screenshot → model → pixel-coordinate action → screenshot. We intend to make this pattern obsolete: it is slow (each step is a screenshot round-trip; Anthropic itself recommends non-real-time use cases; community measurements cite 2–5 s per action), it wastes tokens (megabyte screenshots instead of kilobyte diffs), it is race-prone (the screen can change between observation and action), and it has no authorization model finer than the container boundary.

The token cost of the screenshot loop has been measured by others, and the one figure usually quoted needs stating carefully. A benchmark published by **[Reflex](https://reflex.dev/blog/computer-use-is-45x-more-expensive-than-structured-apis/)** — an enterprise application platform, so a vendor with an interest in the result — and reported by _[The Register](https://www.theregister.com/ai-and-ml/2026/05/07/ai-vision-agents-use-45x-more-tokens-than-apis-in-benchmark/5231346)_ (7 May 2026) found a vision agent consuming "roughly 500,000 input tokens to complete a task that an API agent handled in 12,000 tokens, a 45× cost difference," with the vision path taking ~17 minutes against ~20 seconds. **This is not a measurement of Vitrin, and neither arm of it is Vitrin.** It compares Claude Sonnet driving a *web app* through [browser-use](https://github.com/browser-use/browser-use) 0.12 against Claude Sonnet calling that same web app's own tools and APIs — one task, one vendor, one domain where an API already existed. The API arm wins substantially by not touching the GUI at all, an advantage a display server cannot inherit, because operating real GUIs is the whole point. Read it as an upper bound on what a semantic channel could recover from the screenshot loop, and as motivation for the design — not as a result Vitrin has demonstrated. Vitrin's own numbers are a Phase-2 deliverable (§8) and do not exist yet.

### 1.7 The GUI never had a shell

GUI applications inherited the ambient-authority process model of the terminal era — and then outlived its justification. In the terminal, ambient authority made a kind of sense: the shell is a trusted intermediary that wields the user's full authority, and the user's typed command _is_ the act of designation — `wc report.txt` names exactly the file the program should touch. Capability systems later made this explicit: [Capsicum](https://man.freebsd.org/cgi/man.cgi?query=capsicum) (FreeBSD, 2010), CloudABI, and WASI preopens all implement "the trusted launcher passes programs only the file descriptors designated in argv." The GUI never built its equivalent. Ka-Ping Yee named the missing principle in 2002–2004: **designation is authorization** — the user's act of pointing at a file should itself be the grant. CapDesk (2002) and HP Polaris (CACM, 2006) prototyped it; macOS quietly shipped it at consumer scale — since the App Sandbox era, open/save panels run out-of-process in a daemon literally named Powerbox (`pboxd`), and a sandboxed app receives access only to what the user picked. On the Linux desktop, xdg-document-portal approximates it for Flatpaks, optionally. No display server has ever made it the default law of the system. The terminal has the shell; the GUI never had one. **The core is the GUI's shell.** One consequence deserves headline status: with ambient filesystem access absent by construction, a ransomware payload's blast radius collapses from the home directory to the handful of files the user designated to it (§5.3, P12).

### 1.8 Reachability is authority

The filesystem is not the only ambient authority a confined process inherits — network reachability is the same problem wearing a different mask. A realm that can _reach_ a privileged local service can borrow that service's authority, and the sharpest instance is `ssh localhost`: sshd is a root-privileged confused deputy that vends a full ambient-authority login shell to anyone who authenticates, and the shell it returns runs outside the display server entirely, as the Unix user, with the whole home directory — voiding the empty mount namespace, the powerbox fds, and the physical-input consent in a single hop. It has siblings of identical shape: `flatpak-spawn --host` (an app with the right bus name spawning host processes outside its sandbox), a reachable `docker.sock`/`podman.sock` (equivalent to root), D-Bus activation of a privileged helper, `cron`/`at` job submission, or a setuid binary left inside the namespace. The unifying principle: **a realm's authority is not just what it holds but what it can reach; an unshared network namespace is to sockets what the empty mount namespace is to files.** The draft made the filesystem non-ambient (§1.7, P12) and never did the same for the network — this section closes that asymmetry (P13). Note why [Qubes](https://www.qubes-os.org/) is structurally immune: a qube cannot ssh-escape because dom0 runs no qube-reachable sshd and there is no shared loopback; at the microVM isolation tier (Doc 2 §4.5) "localhost" simply _is_ the VM.

## 2. Vision statement

A display server designed from day zero for a world in which **multiple authenticated principals — humans and AI agents — concurrently observe and operate graphical applications under granular, revocable, capability-scoped authorization.** The core is small and trusted; legacy complexity is exiled to disposable per-app shims; every principal has identity; every action carries a capability; every observation is race-checked; every action is journaled. Network transparency and human supervision are first-class, not afterthoughts.

The terminal has the shell — a trusted intermediary that wields the user's authority and hands programs only what the user designated. The GUI never had one. **This core is the GUI's shell.** And the security story is single: the mechanism that bounds a prompt-injected agent is the same mechanism that bounds ransomware — one architecture, two audiences.

## 3. Target users and personas

1. **Agent-infrastructure builders** (primary persona): teams running fleets of computer-use agents who currently give each agent a whole VM/desktop and want per-surface isolation, dozens of cheap isolated realms per box, and a race-free action API.
2. **Security-conscious enterprises**: regulated organizations wanting an auditable, revocable, least-privilege substrate for agent automation of legacy GUI apps, as an open alternative to WorkSpaces/Windows 365 lock-in.
3. **Agent-framework developers**: authors of frameworks (LangChain, CrewAI, Strands, cua, the OpenAI/Anthropic tool loops) who want a semantic, capability-scoped target instead of pixels and `xdotool`.
4. **Researchers**: HCI, security, and OS researchers working on capability GUIs, agent benchmarks ([OSWorld](https://os-world.github.io/) — 369 real tasks; WindowsAgentArena — 154 tasks with a 74.5% human vs. much lower agent success rate; AndroidWorld), and multi-principal interaction.
5. **(Horizon) Ordinary desktop users**: the session-mode security desktop — ransomware-bounding realms, powerbox file access, wallet-backed logins — is the explicit point of the horizon tier (§5.3), reached only after the greenfield sustains the project.

## 4. Core user stories

1. **50 scoped realms per box (headless fleet).** An operator launches 50 independent agent workloads on one server. Each gets a realm containing exactly the apps it needs; an agent in realm 7 cannot observe or address anything in realm 8; each realm has its own signed journal; the box exposes no shared seat, no shared clipboard, no shared accessibility bus.
2. **Local nested realm alongside a developer desktop.** A developer running Hyprland or GNOME launches the agent-first server _as a window_ (like a VM console). Inside it, an agent operates a scoped set of apps; the developer watches; the agent's activity is confined to that nested surface.
3. **Human-supervised agent with a tinted cursor.** A human and an agent share a realm. The agent's cursor is visually distinct (tinted/tagged). The instant the human moves the physical mouse or types, agent input is preempted by construction. Holding Esc revokes the agent's active grants (dead-man switch).
4. **Audit and deterministic replay.** After an incident, an operator replays a realm's append-only signed journal to see exactly what each principal observed and did, frame by frame, and exports the trajectory as agent training data.
5. **Cross-machine agent driving.** An agent process on machine A drives a UI realm on machine B over an authenticated, multiplexed, capability-scoped session, with the agent's workload identity bound to the transport.
6. **Ransomware containment.** A user runs an untrusted "PDF tools" app in a realm. The realm has no ambient filesystem; the user hands it two PDFs via the core-owned picker. The payload encrypts those two fds and its private realm storage — and nothing else. The home directory was never reachable; the extortion note is an inconvenience, not a catastrophe.
7. **Wallet-backed login and agent delegation.** A native app requests authentication. The core renders an unspoofable consent prompt for an OID4VP presentation of exactly two claims; consent requires physically-originated input, so injected input cannot click Approve. The app receives a short-lived, sender-constrained, realm-audience-bound token — never the credential. Later, an agent working in that realm receives a further-derived, scoped, expiring, journaled token; it never sees a password, because none exists in the flow.
8. **SSH-authenticated fleet operator.** An operator connects to a headless box over SSH. Their SSH certificate's principal maps to a Vitrin OS principal with a scoped grant set — say, observe-and-actuate on realms 1–10 and read-only journals on the rest — not to a root shell. The same SSH key that would have been a skeleton key on a normal box is, here, just one more scoped principal. And `ssh localhost` from _inside_ any realm reaches nothing, because each realm's loopback is its own.

## 5. Scope: invariants, v1, horizon

**Goals:** a new capability-native principal-facing protocol; per-app shim isolation for legacy X11/Wayland; multi-principal input with human preemption; semantic transport (pixel buffer + diffable semantic tree) as a headline pillar; epoch/compare-and-swap action semantics; per-principal signed journals; mediated cross-realm channels; designation-based filesystem and network authority; a credential wallet; app-provenance verification for standing grants; authenticated network sessions; both headless-fleet and local-nested deployment.

Scope statements are of three different kinds, and this document keeps them deliberately separate: **invariants** that hold at every phase, **v1 sequencing choices**, and **horizon** items that are claimed but deferred. A renounced ambition and a deferred one are different promises to the reader.

### 5.1 Invariants (hold forever)

- **No window-management policy, decoration, or theming inside the trusted core.** This is the [Nitpicker](https://os.inf.tu-dresden.de/papers_ps/feske-nitpicker.pdf)/Qubes lesson, not modesty: shell UX lives in unprivileged components, or the TCB bloats and the security argument dies. It is an invariant about _where_ such code runs, never about _whether_ it exists.
- **No app rewrites required.** Legacy X11/Wayland apps run unmodified inside shims; the semantic bridge works from existing accessibility trees. Native integration is always additive, never a precondition.
- **Capability discipline everywhere.** No feature — including every horizon item below — ships with ambient authority, unauditable channels, or bearer credentials.

### 5.2 v1 scope (sequencing decisions, chosen for differentiation-per-effort)

- Headless agent fleets and local nested operation — the two deployments where no open incumbent exists and the pain is acute and current.
- One Wayland shim (the X11 shim follows in Phase 3).
- A minimal unprivileged operator surface: core-rendered consent prompts, a grant-revocation panel, a live journal view.
- One native demo application pushing semantic trees through the native protocol, proving the toolkit-backend path end to end.

### 5.3 Horizon (explicitly claimed, deliberately deferred)

These are _not_ renunciations. Each is a natural extension of the architecture, deferred because each drags a support treadmill that would consume the differentiator if attempted first.

- **Session mode (daily-driver desktop).** Architecturally emergent: the same core on bare DRM/KMS with every app in a shim — no new design, only new support burden (hardware matrix, HDR, color management, fractional scaling, human accessibility, IME for every user). That burden is 90% of the effort for 0% of the differentiator and is what consumed prior alternative display servers. Session mode arrives when the greenfield grows into it, not as a v1 promise. **The security desktop for ordinary users is the _point_ of this horizon, not an accident of it:** bounded ransomware (P12), powerbox file access, and wallet-backed login (P11) are the consumer-facing faces of the same mechanisms v1 builds for agents.
- **Mission-control shell.** Agent-first implies a genuinely new window-manager category: a human supervising N agent realms — a realm grid, tinted per-principal cursors, live journals, grant and consent surfaces. This is the product's face and the project claims it — implemented, per the invariant above, as an unprivileged shell outside the TCB.
- **Native toolkit backends.** A full toolkit is decade-scale and stays out of scope; a toolkit _backend_ is weeks-scale. The project claims Flutter-embedder and Rust-toolkit (iced/egui) backends that push semantic trees natively, seeded by the v1 demo app.
- **Capability remoting protocol.** The network layer (P9) already implies a remoting primitive RDP cannot express: share exactly one window, capability-scoped, audited, revocable. The project claims the _protocol_; the consumer-product grind (codec tuning, NAT traversal, clients for five OSes) is left to the ecosystem — third parties can build "Parsec for realms" on top.
- **Capability shell (research).** A terminal whose shell passes designated fds instead of ambient authority (the Capsicum/CloudABI lineage). v1 deliberately keeps terminal realms ambient — the terminal is the user's direct instrument — and treats a capability shell as long-horizon research.

**Strategic line: maximalist in the greenfield, minimalist toward occupied territory.** In the agent niche there is no incumbent, so there the project can be the whole stack — server, shell, native apps, remoting protocol. On the existing human desktop it rides Wayland via nesting and shims and claims nothing, until the greenfield grows into it.

### 5.4 Hard non-goals (renounced, not deferred)

A full application toolkit; a consumer remote-desktop _product_ (the protocol is claimed, the product is not); displacing Wayland on today's human desktop as a project aim.

## 6. Functional requirements by pillar

**P1 Principals & identity.** N concurrent authenticated principals, each with a distinct identity (human or agent workload), independent focus, and its own cursor (or cursorless). Physical human input MUST preempt agent input by construction. Agent connections MUST present a workload-identity credential at handshake (SPIFFE-SVID- or OIDC-shaped).

**P2 Grants & consent.** No ambient authority anywhere. Every object handle carries explicit rights. A grant is (principal identity × resource × verbs × constraints), where constraints include expiry, event-rate ceilings, and focus-conditions. Grants MUST be **sender-constrained** (bound to the connecting peer identity/socket), not bearer tokens. Persistent grants MUST be modeled like OAuth refresh tokens / portal restore tokens. Consent UI MUST be rendered by the trusted core itself. Revocation MUST be immediate and transitive. Consent persistence is a per-grant **ladder** — deny / once / while-running / until-revoked / always — rendered as such in the consent prompt; the durable rungs (until-revoked, always) MUST be sender-constrained to a **verified app identity** (P14) and appear as live rows in the connected-apps panel with last-used timestamps.

**P3 Realms & shims.** Apps launch _into_ realms. Each legacy app gets its own private nested shim server; N legacy windows = N isolated shim servers. A legacy app's universe contains only itself, so scoping is structural. Realm identity is assigned at shim fork by the core.

**P4 Semantic layer.** Every surface is a pair (pixel buffer + versioned, diffable semantic UI tree). Agents act on semantic nodes, not pixel coordinates. Legacy apps are bridged from accessibility trees; where no tree exists, a server-side cached VLM screen-parsing fallback ([OmniParser](https://github.com/microsoft/OmniParser)-style) synthesizes one. Native toolkits may push trees directly.

**P5 Epochs / CAS.** Every observation (frame + tree) returns an epoch token. Every action carries the expected epoch. The server MUST reject actions whose target changed since observation.

**P6 Journal.** Each principal and each realm has an append-only, signed journal supporting audit and deterministic replay.

**P7 Motion synthesis.** Agents send intent ("click node #42", "drag A→B over 300 ms ease-out"). The server synthesizes trajectories at output refresh rate, independent of client latency.

**P8 Cross-realm flows.** Clipboard, drag-and-drop, file pickers (the designation gesture — see P12), screen sharing, and window activation MUST be deliberate, auditable, permission-gated channels mediated by the core (Qubes-style).

**P9 Network sessions.** Authenticated, multiplexed, capability-scoped remote sessions, with agent workload identity bound to the transport.

**P10 Human overrides.** Visually distinct agent cursors; physical-input preemption; hold-Esc dead-man switch; a connected-apps-style grant-revocation panel; a live journal view.

**P11 Credential wallet (OID4VC/OID4VP).** A display-server-level credential wallet as a separate, hardened, out-of-core privileged service: keys held in hardware (TPM 2.0 / FIDO2 authenticators); credential-format parsing (SD-JWT VC, mdoc) sandboxed outside the core. A presentation is a grant: (principal × credential-claim-subset × present × audience, expiry, one-shot). Selective disclosure maps onto scopes. Audience binding is structural: realm identity is assigned by the core at fork, so app X cannot request as app Y — the [WebAuthn](https://www.w3.org/TR/webauthn-3/) origin-binding property, generalized to the desktop. Consent for presentations is rendered by the core and MAY be constrained to **physically-originated input**, which the core can verify end-to-end — an agent or injected input structurally cannot approve a presentation. Relying apps and agents receive short-lived, sender-constrained, derived tokens; they never receive the credential. Every presentation is journaled. Target interop: OpenID4VC / eIDAS 2.0 EUDI wallets.

**P12 Filesystem authority (designation is authorization).** Realms spawn with **no ambient filesystem authority** (empty mount namespace + [Landlock](https://landlock.io/)). The user's designation gesture — core-owned file picker, drag-and-drop — _is_ the authorization: the picker returns not a path but an already-opened fd delivered over `SCM_RIGHTS`, killing TOCTOU by construction. Directory-level designation ("open project folder") grants a subtree. For path-expecting legacy apps, the shim materializes granted files at synthetic paths (the xdg-document-portal FUSE pattern). Gesture-less programs (backup daemons, indexers) require explicit **standing grants** — first-class, visible, revocable, journaled, persistence rung chosen from the P2 ladder, with the durable rungs gated on verified provenance (P14). Enforcement MUST land at the kernel (namespaces, Landlock, fds), never GUI-only: a compositor never sees `open(2)`, and a GUI-only broker is theater.

**P13 Network authority (reachability is designated, not ambient).** A realm has **no ambient network reachability**: at the default isolation tier it runs in its own network namespace with loopback only, so a reachable-service escape (`ssh localhost`, host-spawn helpers, container sockets, D-Bus activation) finds nothing listening — the attack dies by construction. Own-netns also confines Linux abstract-namespace sockets (bound to the netns); combined with the empty mount namespace hiding path sockets (D-Bus, container sockets), this closes path-sockets, abstract-sockets, and TCP-localhost together. Egress is a **designated capability**, granted like a file: no outbound reachability unless the user or policy designates it, host:port-specific and core-mediated, expiring, revocable, journaled. "Couldn't `ssh localhost`" is simply "localhost:22 was never granted." Enforcement is kernel-level (network namespaces, plus a mediating proxy for designated egress), never application-level. Standing egress grants follow the same P2 ladder and P14 provenance gating. This pillar is the socket-side sibling of P12 (§1.8).

**P14 App provenance (authenticity, not virtue).** Standing grants need to know _who_ they are granted to, so the durable consent rungs are gated on verified provenance. Three properties, kept deliberately distinct: **integrity** (the binary is untampered — signature verification), **continuity** (version N+1 comes from whoever shipped version N — a TOFU key pin, the Android / SSH `known_hosts` model, which by construction trusts v1 blindly and only catches a later switch), and **identity** (this really is the named publisher — an identity-bound certificate, Sigstore-style: a [Fulcio](https://github.com/sigstore/fulcio)-class CA issues short-lived signing certificates bound to an OIDC identity, and every signature is entered in a **transparency log**, with the client verifying a signed checkpoint plus a Merkle inclusion proof as a millisecond local check). Domain-as-identity alone is rejected: "redirect to the app's website" as a trust root is the lookalike-domain trap (`backups-inc.com` vs `backupsinc.com`); a name is meaningful only relative to which issuer vouched for it and what name was expected. **Deliberately not a blockchain:** the property required is a tamper-evident, append-only, publicly auditable record, which a Merkle transparency log provides without consensus, tokens, or a network dependency on the verification hot path — the decade of prior art is [Certificate Transparency (RFC 9162)](https://www.rfc-editor.org/rfc/rfc9162), [Sigstore](https://www.sigstore.dev/)'s [Rekor](https://github.com/sigstore/rekor), Go's checksum database, and the [CONIKS](https://www.usenix.org/conference/usenixsecurity15/technical-sessions/presentation/melara)/Key Transparency line, none of which used a chain, because there is no double-spend to order. DIDs (`did:*`) MAY be accepted as an identity _format_ without putting a ledger on the hot path. Provenance attestations are Verifiable Credentials about software, verified by the same wallet trust engine as human and agent credentials — **one trust engine, three subjects** (Doc 2 §13). The honest boundary: provenance answers "is this genuinely publisher X's app," killing impersonation; it does NOT answer "is publisher X's genuine app safe." Signing is authenticity, not virtue — which is why verified apps still receive _scoped_ grants: verification decides who is trusted; the capability model decides how much that trust can cost.

## 7. Success metrics

Adoption- and artifact-based, not revenue-based:

- **Spec:** a published, versioned protocol spec; external review/commentary; at least one independent implementer expressing intent.
- **Reference implementation:** Phase-1 MVP demonstrated end-to-end; Phase-2 semantic + epochs; Phase-3 network + X11 + fleet.
- **Community:** contributors beyond the author; a funded grant (NLnet/NGI Zero); citations in agent-infra and capability-security discussions.
- **Benchmarks:** demonstrated agent task completion on a scoped realm at lower token cost and higher reliability than the screenshot baseline on an OSWorld-style subset.

## 8. Phased roadmap

- **Phase 0 — Spec & manifesto.** Publish the vision, object model, and wire-protocol draft.
- **Phase 1 — MVP slice.** Trusted core (headless + nested) + one Wayland shim + Firefox in a realm + an agent that captures the realm and injects scoped input, gated by a single grant with consent rendered by the core.
- **Phase 2 — Semantic + epochs.** AccessKit/AT-SPI2 bridge, tree versioning/diffing, epoch/CAS action semantics, VLM fallback; native semantic-protocol demo app; filesystem powerbox v0 (empty-namespace realms, Landlock, fd-granting picker); network authority v0 (per-realm loopback-only network namespace, egress-as-grant); IME workstream begins.
- **Phase 3 — Network + X11 + fleet.** Authenticated network session layer, per-app X11 shim with embedded minimal WM, multi-realm headless fleet mode, journal replay tooling; synthetic-path FUSE layer for path-expecting legacy apps; wallet v0 (presentation-as-grant, physical-input-constrained consent, provenance verification for durable standing grants); mission-control shell v0 (unprivileged: realm grid, grant panel, live journal view).
- **Phase 4 — Horizon.** Session mode on bare DRM/KMS; Flutter/iced/egui semantic backends; capability-remoting protocol hardened for third-party clients; EUDI/OID4VC conformance; capability-shell research. Entered only when Phase 1–3 adoption metrics justify the support treadmill.

## 9. Risks and mitigations

- **Ecosystem gravity.** Wayland is industry-accepted; alternatives struggle. [Arcan](https://arcan-fe.com/) (Björn Ståhl), despite ~13–15 years of work and repeated NLnet/NGI Zero funding, remains — in the words of one Hacker News commenter (Jan 2026) — "software with more fans than productive users"; Ståhl's own blog (26 Jan 2026) admits of a hosted demo that "about two people have managed to find it." _Mitigation:_ do not fight Wayland for the desktop; ride it (run nested inside it, consume its clients via shims). Target the _new_ agent-fleet niche where no open incumbent exists and the pain is acute and current. The scope tiers (§5) encode this posture: maximalist in the greenfield, minimalist toward occupied territory.
- **Semantic-tree quality on hostile apps.** Games, canvas apps, and custom GUIs expose no usable tree. _Mitigation:_ server-side cached VLM parsing fallback; be honest that treeless surfaces degrade to pixel+VLM mode.
- **One-maintainer vacuum (the X.Org-death / Arcan bus-factor lesson).** Even Newton's author explicitly wants the project to avoid "a bus factor of one." _Mitigation:_ spec-first (the protocol outlives any implementation), permissive protocol licensing to invite reimplementation, and early grant funding to sustain more than one contributor.
- **Communication opacity (the Arcan lesson).** Arcan's technical brilliance was repeatedly undercut by writing readers found impenetrable ("I still don't feel like I know what it is," HN). _Mitigation:_ relentless clarity, worked examples, and a runnable MVP over manifesto prose.
- **IME pain.** Input methods inside nested compositors are a known hard problem (uneven `text-input-v3`/`input-method-v2` support; fcitx5/IBus popup-positioning fragility). _Mitigation:_ an explicit Phase-2+ workstream with a documented strategy (Doc 2 §14).
- **Consent fatigue and standing grants.** Powerbox designation makes common flows zero-extra-UX, but gesture-less software (daemons, indexers) re-opens the prompt-or-standing-grant door, and standing grants are where fatigue and over-granting live. _Mitigation:_ directory-level designation to cut prompt counts; standing grants as first-class, visible, revocable, rate-audited objects; defaults tuned so the single-gesture path covers the common 90%; durable rungs gated on verified provenance (P14), so fatigue cannot be exploited by impersonators.
- **Powerbox compatibility edges.** Apps with custom, non-portal file dialogs; atomic-save (write-temp-then-rename) patterns over the FUSE synthetic-path layer — known warts inherited from xdg-document-portal. _Mitigation:_ model the FUSE layer on xdg-document-portal including its documented failure modes; publish a compatibility matrix; fall back to per-app subtree standing grants where FUSE breaks.

## 10. Sustainability and funding

Open-source first. Realistic path: **NLnet / NGI Zero** grants (which fund exactly this class of infrastructure — they funded Arcan-A12), the **Sovereign Tech Fund** (which funds Newton), corporate sponsorship from agent-infra vendors, and — only as a later, optional layer — a hosted control plane for fleet management. The PRD deliberately does not center a business model.

## 11. Licensing recommendation

Split licensing: **the protocol specification and wire definitions under a permissive license** (Apache-2.0 with an explicit patent grant; CC-BY for prose) to maximize reimplementation; **the reference implementation under weak copyleft** (MPL-2.0 or LGPL-3.0) to keep core improvements open while allowing linking from differently-licensed clients; **client SDKs (TypeScript/Python) under Apache-2.0.** This mirrors how the permissively-licensed Wayland _protocol_ coexists with diverse implementations.

> **Executed as MPL-2.0.** The choice this section leaves open was closed by D-016 (`docs/plan/20-decision-log.md`), which also draws the copyleft/permissive line by *derivation* rather than by directory. The root [`NOTICE`](../NOTICE) is the normative path→license map. D-015 records the related decision to file no patents.

---

# DOCUMENT 2 — TECHNICAL ARCHITECTURE

## TL;DR

- **Microkernel-style display server:** a small trusted core implementing a new capability-native protocol; each legacy app confined to its own unprivileged, crashable Wayland/X11 shim; buffers move core-ward as dmabuf handles (zero-copy, one extra IPC hop per frame — the [gamescope](https://github.com/ValveSoftware/gamescope)/Qubes precedent).
- **Capability kernel + grant store** binds a workload identity at connect time and issues sender-constrained, attenuable, expiring grants over an object model of {principal, realm, view, surface, actuator, scene, epoch, journal, grant}; consent is rendered by the core.
- **Three enforcement stacks complete the model:** filesystem authority = empty mount namespace + Landlock + picker-granted fds over `SCM_RIGHTS` (designation is authorization, kernel-enforced); network authority = per-realm network namespace (loopback-only) with egress as a designated, host:port-scoped grant, closing the reachable-service escape class (`ssh localhost`, host-spawn helpers, container sockets) by construction; credential wallet = out-of-core hardened service (hardware-held keys, sandboxed SD-JWT/mdoc parsing) whose presentations are core-mediated grants that can require physically-originated consent. The same wallet trust engine verifies **app provenance** (Sigstore-style identity-bound signing + transparency-log inclusion — deliberately not a blockchain), gating durable standing grants on verified publisher identity: one trust engine for humans, agents, and software.
- **Recommended languages:** Rust for the trusted core and capability kernel (memory safety for a minimized TCB; [Smithay](https://github.com/Smithay/smithay) proven in niri/COSMIC); reuse C/C++ ([wlroots](https://gitlab.freedesktop.org/wlroots/wlroots), gamescope) inside shims where legacy semantics already live; TypeScript and Python for agent SDKs. **Wire protocol:** a Wayland-style fd-passing Unix-socket protocol locally with capability-handle semantics; QUIC for network sessions.

## 1. System overview

```
        physical input / displays (headless: virtual)
                     │
              ┌──────▼───────────────────────────────────┐
              │            TRUSTED CORE (Rust)            │
              │  capability kernel · grant store · scene  │
              │  compositor · input router · motion synth │
              │  journal · consent surface · mediators    │
              └───┬───────────────┬───────────────┬───────┘
   principal-facing│ (new protocol)│ dmabuf+events │
   ┌──────────────▼┐   ┌──────────▼─────┐   ┌─────▼───────────┐
   │ native agent  │   │ Wayland shim   │   │ X11 shim        │
   │ client (SDK)  │   │ (per app)      │   │ (per app + WM)  │
   │ + human input │   │  └ Firefox     │   │  └ legacy Xapp   │
   └───────────────┘   └────────────────┘   └─────────────────┘
```

Principals (human input devices, native agent clients) connect to the core over the principal-facing protocol. Legacy apps never touch the core directly: each is launched with `WAYLAND_DISPLAY`/`DISPLAY` pointing only at its private shim, which is itself an unprivileged client of the core.

## 2. Trusted core & TCB boundary

The core is the entire Trusted Computing Base. Everything else — shims, clients, the VLM parser, window-management policy — is untrusted and outside it. Core responsibilities: capability kernel and grant store; scene graph and compositing; input routing and multi-principal arbitration; server-side motion synthesis; journals; the consent surface; and cross-realm mediators. Design principle borrowed from [Genode](https://genode.org/)'s **Nitpicker** GUI server: a secure GUI multiplexer can be tiny — Nitpicker gives each client its own virtualized session, and the Genode window-manager layering adds only ~3,000 lines to the TCB of graphical apps while keeping decorations and layout in sandboxed components. Qubes states that "our GUI infrastructure introduces only about 2,500 lines of C code (LOC) into the privileged domain (Dom0)" for its cross-VM GUI virtualization — direct evidence that a secure multiplexer's trusted portion stays small. We keep window-management _policy_ (decorations, layout) out of the core, exactly as Nitpicker pushes decoration to a separate window-manager component and Fuchsia's [Scenic](https://fuchsia.dev/fuchsia-src/development/graphics/scenic) keeps window-management policy in a product-level "system shell" outside the compositor. The horizon-tier mission-control shell (PRD §5.3) is exactly such an unprivileged component: realm-supervision UX outside the TCB, alongside per-realm decorators. The wallet service (§13) follows the same rule: credential parsing and key handling live outside the core in a hardened privileged client; the core contributes only the unspoofable consent pixels and channel enforcement.

## 3. Principal-facing protocol

### 3.1 Object model

- **Principal** — an authenticated identity (human or agent workload) with independent focus and cursor state.
- **Realm** — a scope container: a set of surfaces/apps. Grants attach to realms; apps launch into realms; realm identity is assigned at shim fork. (Conceptual sibling of Fuchsia's component "realm," a capability boundary over a subtree.)
- **View** — a principal's window onto a realm's scene (a subtree of surfaces the principal may observe). Modeled on Fuchsia Scenic's per-session View into a global scene graph and Genode's session-local view stack.
- **Surface** — a pixel buffer (dmabuf) paired with a semantic scene node.
- **Actuator** — a capability object representing the right to inject a class of input (pointer, key, text, scroll) into a target, with constraints.
- **Scene / semantic tree** — the versioned, diffable UI tree for a surface.
- **Epoch** — a monotonic token stamping an observation of (frame + tree).
- **Journal** — an append-only signed log handle.
- **Grant** — a capability: (principal × resource × verbs × constraints), sender-constrained.

### 3.2 Wire format decision

**Decision: a Wayland-style binary wire protocol over Unix domain sockets, with `SCM_RIGHTS` file-descriptor passing, for local principals; QUIC for network sessions (§10).** Rationale:

- Capability passing needs unforgeable handles. On a Unix socket, an object handle is a per-connection integer that the peer cannot forge into another connection's namespace — the property Fuchsia Scenic relies on ("Sessions cannot directly refer to resources that belong to other sessions even if they happen to know their id") and that Wayland object IDs already provide. Genuine kernel capabilities (dmabuf fds, memfds, socketpairs to sub-servers) travel as passed fds via `SCM_RIGHTS`.
- We evaluated **Cap'n Proto RPC**: it is capability-oriented, its handles _are_ capabilities, it supports promise pipelining, and it is proven over Unix-socket `socketpair`s for sandboxing in Sandstorm with fd passing in its KJ layer. We reject it as the _local hot path_ because (a) it does not yet implement shared-memory transport in practice, so per-frame buffer data would not be zero-copy, and (b) we want frame buffers to move as dmabuf fds outside the RPC payload entirely. We **do** adopt Cap'n Proto's conceptual model (handles-as-capabilities, attenuation, pipelining), and it remains a reasonable serialization for the network control plane.
- We reject Protobuf/gRPC for the local path (no native fd/capability passing; HTTP/2 framing overhead against a frame deadline) and FlatBuffers (good zero-copy reads, no capability/RPC story). A Wayland-shaped protocol also lets us reuse the enormous body of Wayland tooling and mental models, easing the shim implementation.

### 3.3 Capability handle semantics

Every object reference is a grant-bearing handle scoped to one connection. Handles are **attenuable** (a principal may mint a weaker sub-grant to delegate to a helper) and **revocable** (revoking a parent transitively invalidates children). Handles are **sender-constrained**: the core records peer identity (from the handshake credential and socket peer creds `SO_PEERCRED`) and refuses a handle presented on a different connection. This is the display-server analog of OAuth 2.1 sender-constrained tokens / [FAPI](https://openid.net/wg/fapi/) 2.0's requirement to bind tokens to the client — precisely the anti-bearer-token posture the author's authentication background informs.

### 3.4 fd / dmabuf passing

Frame buffers are never copied through the protocol. A shim imports its app's buffer, exports it as a dmabuf fd, and passes the fd to the core via `SCM_RIGHTS`; the core imports it directly into the compositor. This is the Qubes model (grant-table page sharing rather than pixels over the channel — Qubes deliberately "does not transfer all changed pixels via vchan") and the gamescope model (dmabuf passthrough, direct KMS flip when possible, async Vulkan compute composite otherwise) — both proven to make nested per-app composition performant.

## 4. Shim architecture

### 4.1 One binary, N instances, spawn model

A single shim binary runs in two modes (`--wayland`, `--x11`). The core spawns one shim instance per legacy app in an unprivileged sandbox (namespaces/seccomp), sets the child's `WAYLAND_DISPLAY`/`DISPLAY` to a private socket only that shim serves, assigns the realm identity at fork, and `exec`s the app. Because the app's environment names only its own shim, **no token dance is needed for legacy apps** — scoping is structural, achieved by the Qubes / ChromeOS-sommelier / gamescope precedent of one nested server per isolation unit. This also closes the "all X apps share one XWayland and can keylog each other" hole: each X app gets its _own_ X server instance.

### 4.2 Wayland shim

Built on **wlroots** (C, battle-tested). The shim presents a complete, standards-compliant Wayland environment to exactly one app (as gamescope does per game — "the game is running in its own personal Xwayland sandbox desktop, it can't interfere with your desktop and your desktop can't interfere with it"), handles all the `xdg-shell`/`wl_seat` quirks internally, and forwards only (dmabuf buffer + damage + semantic tree) up to the core. wlroots is chosen for the shim precisely because the legacy semantics we want _outside_ the TCB already exist there, mature.

### 4.3 X11 shim

A per-app minimal rootless X server (Xwayland-derived) plus an _embedded_ minimal window manager inside the shim. The embedded WM is necessary because X apps expect a WM for map/focus/`_NET_WM_*` semantics; placing it inside the shim keeps X legacy fully outside the core (the microkernel principle). This mirrors gamescope's per-game Xwayland sandbox and directly addresses the shared-XWayland keylogging hole.

### 4.4 Buffer, input, and damage paths

- **Buffer (app→shim→core):** app renders → shim imports → dmabuf fd passed to core → core composites. One extra hop vs. a monolithic compositor; zero extra copies.
- **Input (core→shim→app):** core routes a principal's actuated input to the target realm/surface → delivers to the owning shim → shim replays it to its app as ordinary seat input (via the shim's own virtual seat; for X via the shim's XTEST/uinput analog). Emulated-vs-physical distinction is preserved end-to-end using the libei/EIS model.
- **Damage / frame callbacks:** the shim forwards app damage regions upward and relays the core's frame-callback timing downward, so the app throttles correctly to the true output cadence.

### 4.5 Isolation dial (per-realm)

Isolation strength is a per-realm policy, not a global constant, because the residual threat below the GUI layer is the kernel itself: namespace sandboxes share the host kernel, and kernel 0-days pierce them. Three tiers, one identical GUI protocol: (a) **default** — namespaces + [seccomp](https://man7.org/linux/man-pages/man2/seccomp.2.html) + Landlock (cheap, fast, fine for low-risk apps); (b) **hardened** — a user-space kernel (gVisor-class) absorbing syscall surface; (c) **paranoid** — a microVM shim (crosvm/cloud-hypervisor-class) where the shim and app live behind a hypervisor boundary, Qubes-grade isolation for one app. Because the shim protocol is unchanged across tiers, the dial is invisible to apps, agents, and the core's object model. Neither Qubes (all-VM, always) nor Flatpak (all-namespace, always) offers this per-app dial.

Independent of tier, every realm additionally gets its own **network namespace** (loopback-only unless egress is designated — P13, §12), **PID namespace, IPC namespace, and UID**, so the baseline unit is "a container per realm plus the display protocol"; the tiers differ in how much they contain the _shared-kernel_ attack surface above that baseline. This makes explicit where the principal boundary sits relative to the Unix-user boundary (§20.11): the default and hardened tiers still share the host kernel, so the microVM tier is the only one that escapes shared-kernel escape classes — stated plainly rather than pretending namespaces are airtight. Own-netns is also what makes `ssh localhost` and the abstract-socket escapes inert by construction (§1.8, §15).

## 5. Capability kernel & grant store

### 5.1 Identity binding at connect

On connect, a principal presents a credential. For agents: a **[SPIFFE](https://spiffe.io/) SVID** (X.509-SVID or JWT-SVID) or an OIDC-shaped workload token; the core validates it and binds the resulting identity to the connection, alongside kernel `SO_PEERCRED`. This is the display-server entry point for the whole zero-trust agent-identity stack — SPIFFE/SPIRE for workload identity, OAuth 2.1 **[token exchange (RFC 8693)](https://www.rfc-editor.org/rfc/rfc8693)** for scope reduction, and the emerging IETF **AIMS** work on agent authentication ([`draft-klrc-aiagent-auth`](https://datatracker.ietf.org/doc/draft-klrc-aiagent-auth/), "AI Agent Authentication and Authorization", composing [WIMSE](https://datatracker.ietf.org/wg/wimse/about/) + SPIFFE + OAuth 2.0). That draft is moving quickly — it reached **-03 on 6 July 2026**, with authors from AWS, Ping Identity, Okta, OpenAI and Zscaler, and its Security Considerations section, incomplete in the March revision this document originally cited, is now written. We still do not hard-wire it, and the reason is now D-008 rather than any specific gap: an individual draft under active revision is exactly what the pluggable verifier exists to absorb. Re-check its status before Phase 3 commits to a wire-level credential format. The author's OIDC/OAuth/FAPI/WebAuthn/PQC background (the [QAuth](https://qauth.dev) project) shapes this: credentials are validated with the rigor of an API authorization server, sender-constrained by default, with PQC-ready signature agility in the handshake.

**SSH as a principal credential.** SSH is the other credential type that matters for the v1 fleet target, because on a headless box SSH is how an operator or agent actually connects. An SSH connection authenticates an identity that maps to a **principal with grants — never to an unconfined login shell.** SSH CA certificates already carry a `principals` list plus `critical-options`/`extensions` (force-command, source-address), which is a capability-shaped credential; SSH-cert principals therefore slot into the same pluggable verifier beside SPIFFE SVIDs and OIDC tokens (SSH is to the shell what OAuth is to HTTP). The certificate's extensions are informational; the grant table remains authoritative. The reconciling rule: **an SSH identity may become a principal; an SSH session may never become an ambient shell inside a confined context.** Concretely, the SSH front-end terminates into the principal-facing protocol (a scoped session), not into a PTY with the user's ambient authority.

### 5.2 Grant table schema

A grant row: `{grant_id, principal_id, realm_id, resource_ref, verbs[], constraints{expiry, max_event_rate, focus_condition, one_shot?}, persistence(once|while_running|until_revoked|always), provenance_ref?, parent_grant_id?, issued_at, issuer}`. Constraints are enforced in the input router and scene server on every use. `parent_grant_id` gives the attenuation/revocation tree. Durable `persistence` rungs are valid only with a `provenance_ref` to a verified identity (§13); the grant dies the moment the presenting binary's identity no longer matches.

### 5.3 Consent surface

When a principal requests a grant the user has not pre-approved, the **core itself** renders a consent prompt (it owns the screen and input, so the prompt is unspoofable — the Qubes trusted-window-decorator and Nitpicker trusted-labeling principle; in Qubes these labels "are drawn by the trusted Window Manager … and apps running in qubes cannot fake them"). Consent is required while the requesting principal is actively processing, echoing MCP's **elicitation** rule (introduced in the 2025-06-18 revision) that consent prompts appear in-context, never "out of nowhere," and MCP's rule that such channels "MUST NOT be used to request sensitive information." The prompt renders the persistence ladder explicitly (once / while-running / until-revoked / always); durable rungs are offered only when the requester's provenance is verified (§13).

### 5.4 Persistence / restore-token analog

Approved persistent grants are stored and reissued as **restore tokens** modeled directly on xdg-desktop-portal RemoteDesktop/ScreenCast restore tokens and OAuth refresh tokens: opaque, revocable, bound to (principal identity × realm × resource), independently expiring. A "connected apps" panel lists all live and persistent grants; revocation is immediate and transitive.

## 6. Semantic subsystem

### 6.1 AccessKit / AT-SPI2 bridge

Inside each shim, a bridge collects the app's accessibility tree. Preferred source is **AccessKit** (the schema Newton and [COSMIC](https://github.com/pop-os/cosmic-epoch) adopt, derived from Chromium's tree); fallback is AT-SPI2 over a _private_ bus scoped to that shim only (never the session bus — closing the AT-SPI backdoor by construction). Chromium/Electron expose usable trees directly; GTK4/Qt6 via their AccessKit/at-spi backends. The bridge normalizes to our node schema and pushes into the surface's scene node. This mirrors the architecture Newton's author cites in Firefox/Chromium, where "the accessibility APIs are implemented in the main browser process … [and] renderer processes push an accessibility tree to the main process."

**Dependency risk, stated up front.** This pillar rests on work that is neither finished nor ours. Newton's protocols are "not yet finalized" (§2, per LWN's OSSNA 2024 coverage, with prototype implementations only); the AccessKit schema we normalize toward is still evolving; and the AT-SPI2 fallback is the unmaintained path we are explicitly trying to leave. If those schemas move, our node model moves with them. That is an accepted cost of betting on the ecosystem's direction rather than inventing a ninth accessibility schema — but it means the semantic pillar's schedule is not fully under this project's control, and a reader should weight it accordingly against the capability model (§4–5), which depends on nothing external.

### 6.2 Versioning, diffing, stable addressing

Following Newton's push model ("tree snapshots and updates are analogous to frames of visual output … every frame is perfect"), each tree update is atomic and epoch-versioned. Nodes carry stable IDs that survive updates so an agent's reference to node #42 remains valid across redraws (or is explicitly invalidated — §7). Diffs are transmitted as KB-scale deltas, not full trees — the core payoff over MB screenshots.

### 6.3 VLM fallback pipeline

For treeless surfaces (games, canvas, custom GUIs), a server-side, _out-of-TCB_ VLM parser (OmniParser-style) produces a synthetic tree from the pixel buffer, cached and invalidated on damage. This is the same "vision when no API exists" hybrid AWS WorkSpaces adopted (MCP tools when available, vision fallback otherwise), but here the synthetic tree is unified into the same node model so agents use one API.

What share of real surfaces need this fallback is **unknown, and the honest answer may be "most of them"** for the workloads agents are actually pointed at — Electron apps, canvas-heavy web apps, and anything drawing its own widgets. If so, the headline framing weakens from "agents stop paying for pixels" to "agents pay for far fewer pixels, and get structure over the rest" — still the right trade against the screenshot loop, but a different claim than the one that is easy to repeat. Measuring that share across a real app corpus is a Phase-2 task and is not done.

### 6.4 Native semantic protocol sketch

New "agent-first" toolkits push a tree directly. Flutter's `Semantics` tree maps almost 1:1 to our node schema; a toolkit calls `scene.push_tree(surface, tree, epoch)` each frame alongside its buffer commit. Windows UIA and Chromium's tree are the comparison models for node roles/states.

## 7. Epoch / CAS mechanics

Every observation returns an epoch. Every action carries `expected_epoch`. The server rejects an action if the target node's state-relevant version has advanced past `expected_epoch` — optimistic concurrency (compare-and-swap) for GUIs. **Target-invalidating changes** (bump the node's epoch): the target's geometry, role, enabled/visible state, text content for a text target, or its removal/reparenting. **Non-invalidating changes**: unrelated sibling updates, cosmetic repaints, animations of _other_ nodes. **Animation interaction:** a node mid-animation is flagged `in_transition`; actions targeting it either wait for settle or are rejected with a "retry after epoch N" hint, so agents never act on a node still in motion. This is the display-server generalization of WebDriver/CDP/Playwright "stale element reference" — but enforced server-side and universally, not per-framework. We believe the unified frame-and-tree epoch is novel; the closest prior art is browser-automation stale-handle detection.

## 8. Input pipeline

- **Multi-principal routing.** Each principal has independent focus. The router maps an actuation (physical device or agent actuator grant) to a target realm/surface, checks the grant's constraints (rate ceiling, focus condition, expiry), and delivers it to the owning shim's virtual seat.
- **Virtual seats per shim.** Each shim exposes exactly one seat to its app; the core owns the principal→shim-seat mapping. This generalizes the **ext-transient-seat-v1** precedent (short-lived virtual seats for remote users, built for wayvnc) into a per-principal model that replaces the `wl_seat` singleton.
- **Server-side motion synthesis.** Agent intents ("drag A→B, 300 ms, ease-out") are interpolated by the core at output refresh rate; smoothness is independent of agent latency and network jitter.
- **Human preemption.** Physical input carries top priority; when a physical event arrives, in-flight agent actuations to the same focus are preempted and the agent's actuator is transiently suspended (the libei "suspend EI during a password prompt" idea, generalized). Hold-Esc triggers a core-level revoke of the active principal's grants.
- **Rate limiting.** Every actuator grant has an event-rate ceiling enforced in the router; a runaway agent cannot flood input.

## 9. Rendering / compositing path

**Recommendation: Smithay (Rust) for the core compositor.** Justification: the core is the TCB, and a memory-safe compositor eliminates whole vulnerability classes in the most privileged component. Smithay is proven in production by **niri** and **COSMIC** — cosmic-comp shipped in COSMIC 1.0 with Pop!\_OS 24.04 LTS in December 2025 — demonstrating that a Rust/Smithay compositor meets real frame deadlines (one independent write-up measured cosmic-comp "never miss[ing] a vblank over a 60-second" 4K stress test). The core imports shim dmabufs, composites per-realm Views, and outputs to physical displays or, in **headless multi-realm mode**, to virtual outputs (one framebuffer per realm, exposed for capture). In **nested mode**, the core is itself a Wayland client of the host compositor (Hyprland/GNOME), rendering into a single host window — the gamescope nested-session pattern. We reject Go for the core (GC-pause risk against frame deadlines) and C/C++ for the core (TCB memory-safety argument), while explicitly _reusing_ C/C++ (wlroots, gamescope code) inside the untrusted shims.

## 10. Network session layer

**Transport: QUIC** (multiplexed streams, built-in TLS 1.3, connection migration for roaming agents, no head-of-line blocking across surfaces; Rust implementation via [quinn](https://github.com/quinn-rs/quinn)). Each remote principal authenticates with its workload identity (SVID/OIDC) bound to the QUIC/TLS channel; display capability handles are then established over that authenticated session. Buffers that cannot be passed as local fds are encoded with a deferred/data-native codec (the Arcan a12 lesson: "pixel buffers as a last resort, not the default"; compression chosen by connectivity/content/context). Reconnection reuses the crash-resilience idea from Arcan SHMIF — dynamic state is renegotiated on reconnect rather than lost. Latency for remote agents is masked by server-side motion synthesis (the trajectory is computed at the display end, not streamed).

## 11. Cross-realm mediated channels

Each cross-realm flow is a deliberate, auditable channel through the core, modeled on the Qubes secure-clipboard design:

- **Clipboard broker.** No shared clipboard. A copy from realm A places data into a core-held buffer only on an explicit, user-initiated action; a paste into realm B is a second explicit action. This is the Qubes Ctrl-Shift-C/V model — "fully controlled by the user, it cannot be triggered/forced by any" realm — generalized to principals and grants.
- **Drag-and-drop / file pickers / screen sharing / window activation.** Each is a grantable, journaled operation with core-rendered confirmation. File designation flows are the powerbox mechanism detailed in §12.
- **Portal-compat layer.** So existing portal-using apps keep working inside shims, each shim exposes an xdg-desktop-portal backend that proxies portal requests (screencast, file chooser, RemoteDesktop) into core-mediated, grant-checked operations. The app sees a normal portal; the core sees an auditable capability request.

## 12. Filesystem and network authority (powerbox) mechanics

Designation is captured at the GUI; enforcement lands at the kernel — the core never sees `open(2)`, so a GUI-only broker would be theater. Mechanics:

- **Empty-authority spawn.** A realm's mount namespace contains the app, its runtime, and its private realm storage — no home directory, no removable media. Landlock rules pin the remainder (kernel-version-dependent coverage is a tracked caveat).
- **fd-granting picker.** The core-owned picker/drag target resolves the user's designation and returns _already-opened_ fds over `SCM_RIGHTS` (O_RDONLY or O_RDWR per gesture). Handing fds, not paths, structurally kills the TOCTOU races that path-based permission checks suffer.
- **Synthetic paths for legacy apps.** Path-expecting apps get granted files materialized under a per-realm FUSE mount (the xdg-document-portal pattern), including honest inheritance of its known warts: atomic-save via write-temp-then-rename, hardlink tricks, and some `mmap` patterns need explicit handling; the compatibility matrix is public (§20.10).
- **Subtree grants.** Directory designation mounts the subtree ("open project folder" — IDE/git workflows, where Android scoped storage and macOS both landed).
- **Standing grants.** Gesture-less software (backup daemons, indexers) uses standing grants: first-class objects in the grant table, visible in the connected-apps panel, revocable, journaled, rate-audited (§20.9); durable persistence rungs require verified provenance (P14, §13).
- **Ransomware consequence.** With ambient filesystem access absent by construction, a payload's write surface is exactly: its private realm storage plus designated fds/subtrees. This is the kernel-level mechanism beneath the PRD's containment claim (P12, user story 6).

**Network authority (the sibling of the above).** The same designation discipline applies to sockets, because reachability is authority (§1.8):

- **Empty network namespace.** Each realm runs in its own netns with loopback only; no route to the host's `127.0.0.1`, no abstract sockets shared with other realms, no reachable privileged local service. `ssh localhost` inside a realm reaches the realm's _own_ empty loopback — nothing is listening, so the escape is structurally void, not policy-blocked.
- **Designated egress.** Outbound reachability is a grant: the core (or a mediating egress proxy) permits exactly the host:port(s) designated, expiring and journaled — the network analog of the fd-granting picker. A realm with no egress grant has no outbound network at all.
- **Own PID / IPC / UID.** Completing the container-per-realm baseline (§4.5), so `/proc` snooping, SysV IPC, and same-UID `ptrace` do not cross realms.
- **Escape consequence.** The reachable-service escape class (`ssh localhost`, `flatpak-spawn --host`, mounted container sockets, D-Bus activation, `cron`/`at`) is closed by construction, not by blocklist: there is simply nothing to reach (§15 threat row). Designated egress is the only outbound path, and it is host:port-scoped and audited.

## 13. Credential wallet service

A privileged out-of-core client, peer to the mission-control shell, exploiting three properties unique to this architecture: **structural realm identity** (assigned at fork, unforgeable — the audience of a presentation is the realm, the WebAuthn origin-binding analog); **unspoofable core-rendered consent**; and **end-to-end physical-vs-emulated input distinction**, so presentation consent can _require_ physically-originated input — an agent or injected malware structurally cannot click Approve.

- **Key custody:** private keys in TPM 2.0 or FIDO2 hardware; the wallet process never exports them.
- **Format parsing out of the TCB:** SD-JWT VC / mdoc / OID4VC protocol parsing is exactly the kind of complex, attacker-facing code that must never live in a compositor; it runs sandboxed in the wallet service.
- **Presentation as grant:** (principal × claim-subset × present × audience, expiry, one-shot) rows in the same grant table; selective disclosure maps to scopes; every presentation journaled.
- **Derived tokens, never credentials:** relying apps receive short-lived, sender-constrained, realm-audience-bound tokens minted on presentation; agents receive further-derived, scoped, expiring, journaled tokens (the RFC 8693 token-exchange shape). No password exists in the flow to steal or share.
- **Interop target:** OpenID4VC / OpenID4VP; eIDAS 2.0 EUDI wallet timelines (member-state wallets due around end-2026) make a display-server-level holder timely rather than speculative — tracked as a moving caveat.

**Provenance: the wallet pointed at software.** A signed provenance attestation — this artifact digest, this publisher identity, this log inclusion — is a Verifiable Credential about an app, so a daemon presenting itself for a standing grant walks the _same_ verification path as an agent presenting a workload credential or a human presenting an identity claim: issuer trusted, signature valid, identity bound, presence logged. One trust engine, three subjects. Mechanics: Sigstore-style identity-bound short-lived signing certificates (a Fulcio-class CA over OIDC identities); signatures entered in a Rekor-class **transparency log**; client-side verification via signed checkpoint + Merkle inclusion proof — local, millisecond, no ledger on the grant-time hot path (P14 states why this is deliberately not a blockchain). Continuity is a TOFU pin on the publisher identity, which catches a later switch but by construction cannot catch a malicious v1. Trust roots — which issuers, which logs — are configurable per deployment; defaults are a governance question (§20.14).

## 14. IME strategy

Input methods inside nested compositors are a known-hard problem: `text-input-v3`/`input-method-v2` support is uneven across compositors, versions must match between client and compositor (per the fcitx5 docs, "text-input requires the Wayland compositor and the client to use the same version of the protocol"), and fcitx5/IBus candidate-popup positioning inside nested surfaces is fragile. Strategy: (1) each shim implements `text-input-v3` toward its app and runs (or proxies to) an input-method connection; (2) the core routes IME UI (candidate windows) as core-owned surfaces so popups are positioned correctly regardless of the app's nesting; (3) for agents, text entry is a first-class `text` actuator that bypasses IME entirely (an agent sends Unicode directly), confining the IME problem to _human_ input paths; (4) document an XWayland-IME fallback for legacy X apps. Explicitly a Phase-2+ workstream.

## 15. Security analysis (threat model)

| Actor                                                                                                                                | Can do                                                                                                  | Cannot do                                                                                                                                                                                                                                                                                                                               |
| ------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Malicious app in a shim**                                                                                                          | Corrupt/crash its own shim; render hostile pixels in its own surface; exhaust its own realm's resources | Observe/address any other realm, surface, or principal; read another app's buffer or tree; synthesize input elsewhere; reach the session's real seat/clipboard/a11y bus (its universe is only itself)                                                                                                                                   |
| **Ransomware payload in a realm**                                                                                                    | Encrypt its private realm storage and any fds/subtrees the user designated to it                        | Reach the home directory or any undesignated path (ambient filesystem absent by construction); spoof the picker (core-owned pixels); escalate via path races (fds, not paths, are granted)                                                                                                                                              |
| **Reachable-service lateral escape** (`ssh localhost`, `flatpak-spawn --host`, `docker.sock`, D-Bus activation, `cron`/`at`, setuid) | Attempt to reach a privileged local service to borrow its ambient authority                             | Reach any such service by default: own network namespace is loopback-only (P13), path sockets hidden by the empty mount namespace, abstract sockets confined to the netns; egress exists only where designated, host:port-scoped and journaled; an SSH identity maps to a scoped principal, never an ambient shell (§5.1)               |
| **Hijacked / prompt-injected agent**                                                                                                 | Act within exactly the grants it holds (this surface, these verbs, this rate, until expiry)             | Exceed its realm; read surfaces it was not granted (e.g. the password-manager window beside it); persist beyond revocation; act after a human preempts; act on a stale target (epoch/CAS rejects it); trigger a credential presentation (physical-input-constrained consent) — the lethal-trifecta blast radius is bounded to one grant |
| **Malicious agent client (bad credential)**                                                                                          | Attempt connection                                                                                      | Bind an identity it cannot prove (SVID/OIDC validated); reuse another connection's handles (sender-constrained); forge object IDs into another connection's namespace                                                                                                                                                                   |
| **Malicious relying app (wallet)**                                                                                                   | Request a presentation                                                                                  | Receive claims beyond the consented subset; obtain the credential itself (derived tokens only); fake consent (physically-originated input required); replay a token elsewhere (sender-constrained, realm-audience-bound)                                                                                                                |
| **Impersonating daemon / lookalike publisher** (re-signed binary, `backupsinc.com` vs `backups-inc.com`)                             | Present itself for a standing grant under a confusable name                                             | Inherit an existing durable grant (sender-constrained to the verified identity — a mismatch kills the grant); pass verification with a tampered binary (signature) or a silently switched publisher (TOFU pin + transparency-log inclusion); trade on domain resemblance (identity is an issuer-vouched name, not a domain string)      |
| **Compromised shim**                                                                                                                 | Lie about its own app's buffers/tree; misbehave toward its own app                                      | Escape its sandbox into the core (unprivileged, seccomp-confined; isolation dial per §4.5); affect other shims/realms; forge a realm identity (assigned by the core at fork, not claimed by the shim); tamper with the journal (append-only, signed by the core)                                                                        |

The core remains the sole TCB; every actor above is outside it — the Nitpicker/Qubes posture: a small trusted multiplexer, everything else disposable and confined.

## 16. Performance analysis

- **Frame-path hop count:** app → shim → core → display = one extra hop vs. a monolithic compositor, zero extra copies (dmabuf fd passing). This is the cost gamescope pays and the Steam Deck ships in production; gamescope explicitly does "the same thing as steamcompmgr … with less extra copies and latency" and can direct-flip via KMS.
- **Latency budget (target):** at 60 Hz the frame deadline is ~16.7 ms; the extra IPC hop is sub-millisecond for an fd pass; server-side motion synthesis decouples agent/network latency from cursor smoothness entirely. cosmic-comp's measured never-missed-vblank result supports the frame-deadline viability of a Rust/Smithay core.
- **Memory per shim/realm:** dominated by the app itself plus a thin shim; the shim adds a compositor's worth of bookkeeping but no duplicate framebuffers (buffers are shared, not copied). Headless mode adds one virtual framebuffer per realm.
- **Powerbox overhead:** native fd-granted access is zero-overhead at read/write time (ordinary fds); only the legacy synthetic-path FUSE layer pays FUSE costs, and only for path-based access by non-portal apps.
- **Token economics:** semantic-tree diffs are KB-scale vs. MB screenshots. The size of the prize is suggested — not established — by the Reflex vendor benchmark (_The Register_, 7 May 2026): ~500,000 tokens and ~17 minutes for a vision agent against ~12,150 tokens and ~20 seconds for an agent calling the app's own API, on one task. That 45× gap is measured between two systems that are not Vitrin, and its API arm bypasses the GUI entirely, so it bounds what a semantic channel could recover rather than describing what one does. Closing any part of it on Vitrin is a Phase-2 benchmark obligation (§8), against the screenshot baseline, on an OSWorld-style subset — unmeasured today.

## 17. Language & dependency choices per component

| Component                                                                                      | Language                                                       | Rationale                                                                                      |
| ---------------------------------------------------------------------------------------------- | -------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| Trusted core (compositor, capability kernel, grant store, input router, journal, motion synth) | **Rust + Smithay**                                             | TCB memory safety; niri/COSMIC prove production viability at frame deadlines                   |
| Wayland shim                                                                                   | **C + wlroots**                                                | Reuse battle-tested legacy Wayland semantics _outside_ the TCB                                 |
| X11 shim + embedded WM                                                                         | **C/C++** (Xwayland-derived + minimal WM)                      | X legacy belongs outside the TCB, in disposable code                                           |
| VLM parser                                                                                     | **Python** (out-of-TCB service)                                | ML ecosystem; isolation makes its memory-unsafety irrelevant to the core                       |
| Wallet service                                                                                 | **Rust** (out-of-core privileged client)                       | Attacker-facing SD-JWT/mdoc parsing wants memory safety; TPM2/FIDO2 via tss-esapi / ctap2      |
| Provenance verifier                                                                            | **Rust** (sigstore-rs-class client, inside the wallet service) | Checkpoint + Merkle inclusion-proof verification; no network on the grant-time hot path        |
| Synthetic-path FUSE layer                                                                      | **Rust** (fuser)                                               | xdg-document-portal semantics in memory-safe FUSE                                              |
| Egress proxy (designated network)                                                              | **Rust**                                                       | host:port-scoped outbound mediation, journaled; memory-safe network-facing code                |
| SSH front-end (fleet)                                                                          | **Rust** (russh) or OpenSSH `ForceCommand`                     | terminates an SSH identity into a scoped principal session, never a PTY with ambient authority |
| Agent SDKs                                                                                     | **TypeScript + Python**                                        | Meet agent frameworks where they live                                                          |
| Network control plane                                                                          | **Rust** (QUIC via quinn); Cap'n Proto optional for RPC        | Memory-safe transport; capability-shaped RPC                                                   |

## 18. API sketch — example agent session (pseudocode)

```
# 1. Connect with workload identity (sender-constrained from here on)
conn = connect("unix:/run/afd/core.sock",
               credential = spiffe_svid())     # X.509-SVID / JWT-SVID / OIDC

# 2. Request a grant scoped to one surface in one realm
grant = conn.request_grant(
    realm    = "realm-7",
    resource = "surface:firefox.main",
    verbs    = ["observe", "actuate.pointer", "actuate.text"],
    constraints = { expiry: "5m", max_event_rate: "20/s",
                    focus_condition: "surface_focused" })

# 3. Consent: core renders an unspoofable prompt; user approves
grant.await_consent()          # returns a restore token for future sessions

# 4. Observe: get frame + semantic tree + epoch (KB diff, not MB screenshot)
obs  = grant.observe()         # obs.tree, obs.epoch
node = obs.tree.find(role="button", name="Search")

# 5. Act with compare-and-swap on the observed epoch
try:
    grant.actuate(click(node.id), expected_epoch = obs.epoch)
except StaleEpoch:
    obs = grant.observe()      # target changed since observation; re-observe & retry

# 6. Every step is appended to the realm's signed journal automatically.
#    A human moving the mouse, or hold-Esc, preempts/revokes instantly.

# 7. Powerbox: the user designates a file in the core-owned picker; the
#    realm receives an already-opened fd — no path, no ambient filesystem.
fd = grant.request_file(mode="rw")             # blocks on user designation

# 8. Wallet: request a presentation; consent requires PHYSICAL input, so
#    this call can never be self-approved by the agent.
tok = conn.wallet.present(claims=["age_over_18"], audience="realm-7",
                          expiry="10m", one_shot=True)
#    -> short-lived, sender-constrained token; the credential never leaves the wallet.

# 9. Network egress is designated too: no outbound reachability until granted,
#    host:port-scoped — the socket analog of request_file.
sock = grant.request_connect(host="api.example.com", port=443)   # blocks on designation
#    'ssh localhost' inside this realm reaches an empty loopback: nothing is listening.

# 10. Gesture-less daemon: presents a provenance attestation (a VC about
#     software), then requests a durable standing grant — the same trust
#     engine as steps 1 and 8. One trust engine, three subjects.
att = conn.wallet.verify_provenance(artifact_digest, publisher="backups-inc")
sg  = conn.request_grant(realm="realm-3", resource="subtree:/backups",
                         verbs=["read"], constraints={persistence: "until_revoked"},
                         provenance=att)   # durable rung requires verified identity
```

## 19. MVP cutline mapped to roadmap

- **Phase 1 (MVP):** core (nested + headless) · one Wayland shim (wlroots) · Firefox in realm-7 · agent connects with a static identity · one grant (observe + pointer + text) · core-rendered consent · pixel capture + scoped input inject. _Out of MVP:_ semantic trees, epochs, network, X11, multi-realm fleet, powerbox, wallet.
- **Phase 2:** AccessKit/AT-SPI bridge · tree diff/versioning · epoch/CAS · VLM fallback · native semantic-protocol demo app · filesystem powerbox v0 (empty-namespace realms, Landlock, fd-granting picker) · network authority v0 (per-realm loopback-only netns, egress-as-grant) · IME workstream begins.
- **Phase 3:** QUIC network sessions · X11 shim + embedded WM · headless multi-realm fleet · journal replay + training-data export · cross-realm mediators hardened · synthetic-path FUSE layer · wallet v0 (presentation-as-grant, physical-input-constrained consent, provenance verification for durable standing grants) · mission-control shell v0 (unprivileged).
- **Phase 4 (Horizon):** session mode on bare DRM/KMS · Flutter/iced/egui semantic backends · capability-remoting protocol hardened for third-party clients · EUDI/OID4VC conformance · capability-shell research.

## 20. Open questions (honestly listed)

1. **Epoch granularity vs. animation-heavy UIs:** how coarse can node-epoch invalidation be before either false-rejecting valid actions or admitting stale ones? Needs empirical tuning on real apps.
2. **Semantic node-addressing stability** across app-driven tree rebuilds (SPA-style full re-renders) — how much can the bridge do without app cooperation?
3. **VLM fallback trust and cost:** the parser is out-of-TCB, but a wrong synthetic tree causes misclicks; how is its confidence surfaced to agents?
4. **Grant delegation depth:** should attenuated sub-grants be bounded in chain length (the IETF AIMS "delegation-chain-depth" open problem)?
5. **Portal-compat coverage:** how many real portal-using apps work unmodified inside a shim's proxied portal backend?
6. **Network buffer codec:** which codec/latency profile for remote realms without a local GPU on the sink side?
7. **Identity-standard churn:** agent-identity standards (IETF AIMS, MCP authorization at revision 2025-06-18 and beyond) are moving targets in 2026; how much to commit vs. abstract behind a pluggable verifier?
8. **Bus factor:** how to get beyond a single maintainer before the reference implementation becomes load-bearing.
9. **Standing-grant ergonomics:** how do gesture-less daemons get authority without re-opening consent fatigue — grant templates? install-time manifests reviewed once? rate-audited defaults?
10. **Atomic-save over synthetic paths:** which save patterns (rename-over, hardlink, mmap) survive the FUSE layer unmodified, and which need shim-side emulation — empirically, against the most-used desktop apps?
11. **Principal boundary vs Unix-user boundary:** for v1, is a realm intra-user (own netns/PID/IPC/UID, shared kernel) sufficient, or should the default be per-UID, with the microVM tier reserved for untrusted apps? The default and hardened tiers share the host kernel; only the microVM tier escapes shared-kernel escape classes (§4.5). This is the boundary question the `ssh localhost` example surfaced.
12. **Egress-designation ergonomics:** how does an app that legitimately needs many hosts (a browser realm) get outbound reachability without either a blanket grant that guts the model or prompt fatigue — per-realm-template allowlists, trust-on-first-use with journal, or a categorized egress policy?
13. **Human factors of the ladder:** users click "always allow" without reading, and no cryptography fixes that. What prompt design, defaults, and rate-of-asking keep the durable rungs meaningful — and what must the journal surface to make post-hoc review actually happen?
14. **Trust-root governance:** who operates the transparency log(s), and which identity issuers are trusted by default — a project-run log, federation with Sigstore's public-good instances, or per-deployment roots? Default choices here shape the ecosystem's neutrality.

---

## Naming (decided)

**Project name: Vitrin OS.** The original top pick was **Kavşak** (Turkish, "junction/intersection" — evokes the point where many principals meet a shared surface), but it was dropped in favor of **Vitrin** (Turkish, "display window/showcase") because "ş" is a hard sound and character for non-Turkish speakers to say, type, or recall — a real cost for a project whose growth depends on organic, English-first developer pickup (HN, conference mentions, casual word of mouth) rather than a marketing budget. "Display/showcase" is also the more literal metaphor for a *display* server than "junction."

- The bare word `vitrin` was already squatted on npm (an abandoned, unrelated "Storyboards cli" package last published 2022) by the time namespaces were claimed, so the project identity was extended to **Vitrin OS** / `vitrin-os` for a name that is claimable and consistent across every registry.
- **Claimed namespaces (12 July 2026):** GitHub org `github.com/vitrin-os`; npm scope `@vitrin-os/*`; crates.io crates `vitrin-os` and `vitrind` (the trusted-core daemon binary, Doc 2 §2 §9) — all reserved with placeholder publishes ahead of any public announcement.
- **Torii** (Japanese, "gateway") was considered — fits the mediated-channel/gateway design, a tasteful nod to the author's life in Japan — but not adopted; recorded here in case naming is revisited.
- **Avoided:** _Newton_ (GNOME a11y), _Scenic_/_Nitpicker_/_Arcan_/_Genode_ (taken); religiously-connoted names (e.g. _Mihrab_) — awkward for a systems project.

---

## Caveats

- **Forward-looking / preview technologies.** Microsoft MXC and Windows 365 for Agents, and parts of the AWS WorkSpaces agent feature set, were announced/GA'd in mid-2026; some sub-features (micro-VM/Linux-container MXC backends; Agent 365 policy integrations) are on published roadmaps and "in preview," not shipped. Treat competitive comparisons as of June–July 2026.
- **The epoch/CAS mechanism is a design claim, not a proven result.** We believe the unified frame-and-tree epoch is novel, but its optimal granularity is an open empirical question (§20.1); the closest validated prior art is per-framework stale-element detection in browser automation.
- **Newton and Wayland accessibility are moving targets.** Newton's protocols are "not yet finalized" (LWN, OSSNA 2024 coverage) and it is prototype-stage; our AccessKit-based bridge depends on schema stabilization we do not control.
- **Agent-identity standards are unsettled.** The IETF AIMS draft (2 March 2026) leaves its Security Considerations as "TODO," and the MCP authorization spec has evolved across revisions; we deliberately abstract identity verification behind a pluggable verifier rather than hard-committing.
- **Wallet/eIDAS timelines are moving.** EUDI member-state wallet availability (~end-2026) and OpenID4VC/OpenID4VP revision churn mean interop specifics are as-of mid-2026; the wallet service sits behind the same pluggable-verifier discipline as agent identity.
- **Landlock coverage is kernel-dependent.** Filesystem-authority guarantees assume Landlock ABI levels present in current kernels; older kernels degrade to namespace-only enforcement — documented per isolation tier.
- **FUSE synthetic-path warts are inherited knowingly** from the xdg-document-portal pattern (atomic-save/rename, hardlinks, some mmap); the compatibility matrix (§20.10) is the honest ledger.
- **The Qubes "~2,500 lines" figure** is Qubes' own published claim about its GUI-virtualization code in Dom0; treat it as vendor self-reporting rather than an independent audit. Genode's "~3,000 lines added to the TCB" is likewise the project's own figure.
- **Arcan-adoption reasoning** relies substantially on community commentary (Hacker News) and the maintainer's own blog admissions rather than a formal post-mortem; it is representative sentiment, directionally reliable, not a controlled study.
- **Performance numbers are indicative.** The gamescope/Qubes precedents establish that per-app nested composition with dmabuf passthrough is viable and low-overhead, but no benchmark of _this_ architecture exists yet; latency and memory figures are design targets to be validated against the Phase-1 MVP.
- **Scope tiers matter.** v1 is deliberately not a daily-driver desktop, a toolkit, or a remote-desktop product. Session mode, toolkit backends, and the capability-remoting protocol are horizon-tier claims (PRD §5.3), not renunciations; a full toolkit and a consumer remoting product are renounced outright (§5.4). Evaluating v1 against horizon yardsticks would misjudge it.
- **Same-user confinement is the weakest tier.** At the default and hardened isolation tiers a realm shares the host kernel with the core and other realms; namespaces (mount, net, PID, IPC, UID), Landlock, seccomp, and loopback-only networking bound but do not eliminate shared-kernel escape — a kernel 0-day pierces them. Only the microVM tier (§4.5) escapes that class. Security claims are stated per tier, not as a single absolute (§20.11). The `ssh localhost`-class escapes are closed at _every_ tier by own-netns; the residual risk this caveat names is the kernel beneath the namespace, not the reachable-service vector.
- **Provenance infrastructure for desktop Linux is uneven.** Sigstore-style identity-bound signing and transparency logs are mature for supply-chain artifacts, not yet the norm for desktop app distribution (Flathub signs repositories, not per-publisher identity-bound certificates); P14 assumes tooling that exists but is not yet ambient. TOFU continuity carries its known blind spot: a malicious v1 is trusted until it misbehaves — provenance is authenticity, not virtue, and the scoped-grant model is what bounds the cost of that gap.

---

## License

This document's prose is licensed under [CC BY 4.0](../LICENSE-CC-BY-4.0),
per decision D-005 (`docs/plan/20-decision-log.md` — spec prose is
permissively licensed independently of the reference implementation). See
the repository root [`NOTICE`](../NOTICE) for the full license split.
