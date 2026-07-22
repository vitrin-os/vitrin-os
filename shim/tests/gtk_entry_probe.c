/* gtk_entry_probe.c -- the R5 gate's app: a real GTK text entry, running as
 * a real Wayland client of the shim, that prints back exactly what landed in
 * it.
 *
 * WHY GTK AND NOT `input_echo_client.c`. The echo client proves the shim
 * delivered the right keysyms, because it resolves them through the keymap
 * itself with xkbcommon. That is necessary but not sufficient for the
 * acceptance criterion, which is about a TEXT FIELD: between `wl_keyboard.key`
 * and a character appearing in a widget sits a whole input-method layer --
 * GTK's `GtkIMContextSimple`, its compose table, its dead-key handling, its
 * own idea of which keysyms are text and which are commands. A synthesized
 * keymap can be correct at the xkb level and still produce nothing at the
 * widget level. This program is the only way to tell those two apart, which
 * is precisely why the criterion names a GTK text field rather than a
 * keysym stream, and why R5 puts it ahead of Firefox.
 *
 * WHAT IT DOES. Shows a window with a single `GtkEntry`, focuses it, waits
 * `--run-ms`, then prints
 *
 *   ENTRY <the entry's contents, one line, verbatim UTF-8>
 *   ENTRY_HEX <the same bytes in hex>
 *   ENTRY_CHARS <codepoint count>
 *
 * and exits. The hex line is not decoration: a mangled character and a
 * correct one can look identical in a terminal with the wrong font, and the
 * assertion has to be on bytes.
 *
 * GTK3 rather than GTK4 by default, because GTK3's `GtkEntry` is the widget
 * in the largest number of the legacy apps this shim exists to confine; the
 * build wires up whichever the machine has (see meson.build).
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <glib-unix.h>
#include <gtk/gtk.h>

static GtkWidget *g_entry;
static int g_run_ms = 4000;

static void dump_and_quit(void) {
	const char *text = "";
#if GTK_MAJOR_VERSION >= 4
	GtkEntryBuffer *buf = gtk_entry_get_buffer(GTK_ENTRY(g_entry));
	text = gtk_entry_buffer_get_text(buf);
#else
	text = gtk_entry_get_text(GTK_ENTRY(g_entry));
#endif
	if (text == NULL) {
		text = "";
	}
	printf("ENTRY %s\n", text);
	printf("ENTRY_HEX ");
	for (const unsigned char *p = (const unsigned char *)text; *p != '\0'; p++) {
		printf("%02x", *p);
	}
	printf("\n");
	printf("ENTRY_CHARS %ld\n", (long)g_utf8_strlen(text, -1));
	fflush(stdout);
	exit(0);
}

static gboolean on_timeout(gpointer data) {
	(void)data;
	dump_and_quit();
	return G_SOURCE_REMOVE;
}

/* On-demand dump (SIGUSR1). The `--run-ms` timer is a fixed deadline from
 * startup, which races a driver that must first bring the window up, render it,
 * and inject a stimulus before the entry is meant to be read -- the P1.8.6
 * agent-actuation gate (test_real_actuation.py) does exactly that, and under a
 * slow CI cold-start GTK render the timer could fire before the typed string
 * lands. So a driver that knows WHEN it has finished typing can trigger the
 * dump precisely then, instead of guessing a timeout that outlasts every
 * runner. Delivered through `g_unix_signal_add`, so it runs in the GTK main
 * loop (not a raw handler) and `dump_and_quit`'s GTK calls are safe. SIGUSR1 is
 * used by no other caller of this probe -- the P1.6.3 seat-replay gate drives
 * it purely on the timer -- so adding this path changes nothing for them. */
static gboolean on_dump_signal(gpointer data) {
	(void)data;
	dump_and_quit();
	return G_SOURCE_REMOVE;
}

/* Report the moment the window is actually on screen, so the harness can see
 * in the log that the stimulus was not fired at a window that did not exist
 * yet -- the single most likely way this test would lie. */
static gboolean on_mapped(gpointer data) {
	(void)data;
	printf("PROBE mapped\n");
	fflush(stdout);
	return G_SOURCE_REMOVE;
}

int main(int argc, char **argv) {
	for (int i = 1; i < argc; i++) {
		if (strcmp(argv[i], "--run-ms") == 0 && i + 1 < argc) {
			g_run_ms = atoi(argv[++i]);
		}
	}

#if GTK_MAJOR_VERSION >= 4
	gtk_init();
	GtkWidget *window = gtk_window_new();
	g_entry = gtk_entry_new();
	gtk_window_set_child(GTK_WINDOW(window), g_entry);
	gtk_window_set_default_size(GTK_WINDOW(window), 480, 120);
	gtk_window_present(GTK_WINDOW(window));
	gtk_widget_grab_focus(g_entry);
#else
	gtk_init(&argc, &argv);
	GtkWidget *window = gtk_window_new(GTK_WINDOW_TOPLEVEL);
	g_entry = gtk_entry_new();
	gtk_container_add(GTK_CONTAINER(window), g_entry);
	gtk_window_set_default_size(GTK_WINDOW(window), 480, 120);
	gtk_widget_show_all(window);
	gtk_widget_grab_focus(g_entry);
#endif

	g_idle_add(on_mapped, NULL);
	g_timeout_add((guint)g_run_ms, on_timeout, NULL);
	/* On-demand dump for a driver that knows when it has finished typing; the
	 * timer above stays as the standalone/fallback deadline. */
	g_unix_signal_add(SIGUSR1, on_dump_signal, NULL);

#if GTK_MAJOR_VERSION >= 4
	while (g_list_model_get_n_items(gtk_window_get_toplevels()) > 0) {
		g_main_context_iteration(NULL, TRUE);
	}
#else
	gtk_main();
#endif
	dump_and_quit();
	return 0;
}
