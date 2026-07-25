// Deterministic demo/test profile for the pinned Firefox ESR (P1.6.4).
//
// SPDX-License-Identifier: MPL-2.0
//
// Copied to <profile>/user.js by the acceptance script, which creates a fresh
// profile per run -- a profile that persists across runs accumulates session
// state, and "deterministic" then means "deterministic until someone runs it
// twice".
//
// FOUR THINGS THIS FILE EXISTS TO GUARANTEE, in order of how badly each one
// would corrupt a test:
//
//   1. NO NETWORK. Not "less network" -- none. A test whose result depends on
//      reaching Mozilla's servers is a test that goes red when a CDN hiccups,
//      and a red test nobody believes is worse than no test. The blunt
//      instrument is the proxy: everything remote is pointed at 127.0.0.1:1,
//      where nothing listens, so every remote request fails at connect
//      immediately and deterministically instead of hanging for a timeout.
//      The individual feature switches below are still set, because a
//      request that is never issued costs nothing at all, and because the
//      list doubles as documentation of what Firefox would otherwise phone
//      home about. The pages under test are file:// URLs, which the proxy
//      does not touch.
//   2. NO FIRST-RUN UI. The welcome tour, the "what's new" page and the
//      default-browser prompt all paint over the content area -- which is the
//      thing being measured. A dominant-colour assertion against a window
//      that is showing an onboarding modal fails for a reason that has
//      nothing to do with the shim.
//   3. NO UPDATE MACHINERY. An update check that succeeds is a network
//      dependency; one that half-succeeds can restart the browser mid-run.
//      The pin (firefox-esr.pin) is meaningless if the binary can update
//      itself out from under it.
//   4. SOFTWARE WEBRENDER. See shim/docs/firefox.md -- this is a documented
//      supported configuration, not a workaround, because the shim's headless
//      pixman path has no GPU by construction and CI never will.

// --- 4. software WebRender (the GPU-less rendering path) -----------------
// gfx.webrender.software forces WebRender's CPU backend (SWGL). The
// .all + .force-* pair keeps Firefox from falling back further, to the
// legacy Basic compositor, on a machine where it cannot probe a GPU: that
// fallback still paints, but it is a different code path from the one the
// demo and CI are meant to exercise, and it would drift silently.
user_pref("gfx.webrender.software", true);
user_pref("gfx.webrender.all", true);
user_pref("gfx.webrender.force-disabled", false);
user_pref("layers.acceleration.disabled", true);
user_pref("webgl.force-enabled", false);
user_pref("media.hardware-video-decoding.enabled", false);

// --- 1. no network -------------------------------------------------------
user_pref("network.proxy.type", 1);
user_pref("network.proxy.http", "127.0.0.1");
user_pref("network.proxy.http_port", 1);
user_pref("network.proxy.ssl", "127.0.0.1");
user_pref("network.proxy.ssl_port", 1);
user_pref("network.proxy.socks", "127.0.0.1");
user_pref("network.proxy.socks_port", 1);
user_pref("network.proxy.no_proxies_on", "");
user_pref("network.proxy.allow_hijacking_localhost", true);
user_pref("network.dns.disablePrefetch", true);
user_pref("network.prefetch-next", false);
user_pref("network.captive-portal-service.enabled", false);
user_pref("network.connectivity-service.enabled", false);
user_pref("network.trr.mode", 5);
user_pref("toolkit.telemetry.enabled", false);
user_pref("toolkit.telemetry.unified", false);
user_pref("toolkit.telemetry.archive.enabled", false);
user_pref("toolkit.telemetry.server", "");
user_pref("datareporting.healthreport.uploadEnabled", false);
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("app.normandy.enabled", false);
user_pref("app.shield.optoutstudies.enabled", false);
user_pref("browser.ping-centre.telemetry", false);
user_pref("browser.discovery.enabled", false);
user_pref("browser.safebrowsing.malware.enabled", false);
user_pref("browser.safebrowsing.phishing.enabled", false);
user_pref("browser.safebrowsing.downloads.enabled", false);
user_pref("browser.safebrowsing.blockedURIs.enabled", false);
user_pref("browser.region.network.url", "");
user_pref("browser.region.update.enabled", false);
user_pref("extensions.getAddons.cache.enabled", false);
user_pref("extensions.blocklist.enabled", false);
user_pref("media.gmp-manager.updateEnabled", false);
user_pref("media.gmp-provider.enabled", false);
user_pref("identity.fxaccounts.enabled", false);
user_pref("browser.contentblocking.report.hide_vpn_banner", true);

// --- 2. no first-run UI --------------------------------------------------
// homepage_override.mstone = "ignore" is the specific switch that suppresses
// the post-upgrade "what's new" tab; without it a FRESH profile on a pinned
// build still shows one, because "fresh profile" reads as "upgraded from
// nothing".
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("browser.startup.page", 0);
user_pref("browser.startup.homepage", "about:blank");
user_pref("browser.startup.firstrunSkipsHomepage", true);
user_pref("browser.aboutwelcome.enabled", false);
user_pref("browser.messaging-system.whatsNewPanel.enabled", false);
user_pref("browser.uitour.enabled", false);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.shell.didSkipDefaultBrowserCheckOnFirstRun", true);
user_pref("browser.newtabpage.enabled", false);
user_pref("browser.newtabpage.activity-stream.feeds.section.topstories", false);
user_pref("browser.newtabpage.activity-stream.showSponsoredTopSites", false);
user_pref("browser.privatebrowsing.vpnpromourl", "");
user_pref("startup.homepage_welcome_url", "");
user_pref("startup.homepage_welcome_url.additional", "");
user_pref("startup.homepage_override_url", "");
user_pref("trailhead.firstrun.didSeeAboutWelcome", true);
user_pref("datareporting.policy.firstRunURL", "");

// --- 3. no update machinery ---------------------------------------------
user_pref("app.update.auto", false);
user_pref("app.update.enabled", false);
user_pref("app.update.checkInstallTime", false);
user_pref("app.update.disabledForTesting", true);
user_pref("app.update.background.scheduling.enabled", false);
user_pref("extensions.update.enabled", false);
user_pref("extensions.update.autoUpdateDefault", false);

// --- crash / session determinism ----------------------------------------
// A run that ends by being killed must not make the NEXT run open a session
// restore page instead of the page it was told to open. max_resumed_crashes
// = -1 disables the safe-mode-after-crashes prompt entirely.
user_pref("browser.sessionstore.resume_from_crash", false);
user_pref("browser.sessionstore.max_resumed_crashes", 0);
user_pref("toolkit.startup.max_resumed_crashes", -1);
user_pref("browser.tabs.crashReporting.sendReport", false);
user_pref("browser.tabs.warnOnClose", false);
user_pref("browser.warnOnQuit", false);
user_pref("browser.sessionstore.interval", 60000);

// --- URL bar determinism -------------------------------------------------
// The P1.6.4 acceptance types a file:// URL into the URL bar and presses
// Return. keyword.enabled=false stops a typo becoming a SEARCH (which, with
// the proxy above, would be an error page rather than the target page and
// would fail the assertion for the wrong reason); the suggestion prefs stop
// an autocomplete dropdown from painting over the content area between the
// keystrokes and the Return.
user_pref("keyword.enabled", false);
user_pref("browser.fixup.alternate.enabled", false);
user_pref("browser.urlbar.suggest.searches", false);
user_pref("browser.urlbar.suggest.history", false);
user_pref("browser.urlbar.suggest.bookmark", false);
user_pref("browser.urlbar.suggest.openpage", false);
user_pref("browser.urlbar.suggest.topsites", false);
user_pref("browser.urlbar.suggest.quicksuggest.sponsored", false);
user_pref("browser.urlbar.autoFill", false);
user_pref("browser.urlbar.maxRichResults", 0);
