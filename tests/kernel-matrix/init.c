// SPDX-License-Identifier: Apache-2.0
//
// PID 1 for the per-kernel isolation matrix's guest (issue #281).
//
// It exists to run the SHIPPED `vitrind` on a distribution's own unmodified
// vmlinuz and to make every way that could fail visible from outside. The
// whole hazard this file is written against is a boot that produces plausible
// silence: a kernel that panics, an init that cannot mount /proc, a `vitrind`
// that is not in the image at all. None of those may look like a measurement,
// so every phase is fenced by a sentinel the collector requires, and anything
// that goes wrong before the fences close prints `VITRIN-FATAL` and powers the
// machine off rather than continuing with a probe that would read the wrong
// thing.
//
// Why /proc must be fatal rather than warned about: `Report::probe` reads
// `/proc/sys/user/max_user_namespaces` and `/proc/self/attr/current` among
// others, and a probe over a missing /proc renders `unset`/`absent` cells --
// which is a *reading* an unwary reader would take as a fact about the kernel.
// The one honest response to a failed mount is to refuse to measure.
//
// Static, freestanding of any distribution userspace: the collector compiles
// this with `-static` so the image needs no loader for PID 1 itself. The only
// dynamic thing in the image is `vitrind` and its own library closure, copied
// from the host that built it.
#define _GNU_SOURCE
#include <errno.h>
#include <linux/reboot.h>
#include <stdio.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/reboot.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

// Every phase's child gets this long before SIGALRM takes it down. A `vitrind`
// that hangs must not hold the guest until the collector's own timeout, because
// that failure mode reads as "the boot never finished" rather than as "the
// binary hung".
#define PHASE_TIMEOUT_SECONDS 20

static void fatal(const char *what) {
    printf("VITRIN-FATAL %s: %s\n", what, strerror(errno));
    fflush(stdout);
    sync();
    reboot(LINUX_REBOOT_CMD_POWER_OFF);
    for (;;) pause();
}

// Run one `vitrind` invocation between sentinels, and report its exit status.
//
// The status is reported, never interpreted here: which statuses are legitimate
// is the collector's decision, and an init that decided it would be a second
// place the rules live.
static void phase(const char *name, char *const argv[], char *const envp[]) {
    printf("VITRIN-PHASE-BEGIN %s\n", name);
    fflush(stdout);
    pid_t pid = fork();
    if (pid < 0) fatal("fork");
    if (pid == 0) {
        alarm(PHASE_TIMEOUT_SECONDS);
        execve(argv[0], argv, envp);
        // Reached only when the image does not hold the binary at all -- the
        // "collector ran, measured nothing, and said so" case. 127 is the
        // shell's convention for it and the collector rejects any nonzero
        // status on the probing phases.
        printf("VITRIN-EXEC-FAILED %s: %s\n", argv[0], strerror(errno));
        fflush(stdout);
        _exit(127);
    }
    int status = 0;
    if (waitpid(pid, &status, 0) < 0) fatal("waitpid");
    fflush(stdout);
    if (WIFEXITED(status)) {
        printf("VITRIN-PHASE-END %s exit=%d\n", name, WEXITSTATUS(status));
    } else if (WIFSIGNALED(status)) {
        printf("VITRIN-PHASE-END %s signal=%d\n", name, WTERMSIG(status));
    } else {
        printf("VITRIN-PHASE-END %s unknown\n", name);
    }
    fflush(stdout);
}

int main(void) {
    // setvbuf before the first print: the console is a serial port and a
    // half-buffered line at power-off is a truncated row.
    setvbuf(stdout, NULL, _IONBF, 0);
    setvbuf(stderr, NULL, _IONBF, 0);

    mkdir("/proc", 0755);
    mkdir("/sys", 0755);
    mkdir("/dev", 0755);
    if (mount("proc", "/proc", "proc", 0, NULL) != 0) fatal("mount /proc");
    if (mount("sysfs", "/sys", "sysfs", 0, NULL) != 0) fatal("mount /sys");
    if (mount("devtmpfs", "/dev", "devtmpfs", 0, NULL) != 0) fatal("mount /dev");
    // The startup phase names $XDG_RUNTIME_DIR; give it a real one so a
    // refusal there is about isolation rather than about a missing directory.
    mkdir("/run", 0755);
    if (mount("tmpfs", "/run", "tmpfs", 0, NULL) != 0) fatal("mount /run");
    mkdir("/run/user", 0755);
    mkdir("/run/user/0", 0700);

    // The LEADING newline is not cosmetic. SeaBIOS's "Booting from ROM.." and
    // the kernel's own console reset (`ESC c ESC [?7l ESC [2J`) are written to
    // the same serial port with no trailing newline, so on some kernels the
    // first byte this process writes lands on THAT line -- measured on
    // 5.15.0-191-generic, where the log read `Booting from
    // ROM..<esc>VITRIN-INIT-BEGIN`. The collector anchors its sentinels to the
    // start of a line, so without this the first fence never matches and a
    // perfectly good boot is reported as a dead one. It failed in the safe
    // direction, but a harness that cries wolf gets its checks deleted.
    printf("\nVITRIN-INIT-BEGIN\n");

    // NO_COLOR: `vitrind` honours it by hand (see `init_tracing`), and without
    // it the startup phase's lines arrive wrapped in ANSI escapes that would
    // land in a checked-in row.
    char *envp[] = {"PATH=/usr/bin:/bin:/usr/sbin:/sbin", "HOME=/", "NO_COLOR=1",
                    "XDG_RUNTIME_DIR=/run/user/0", "RUST_LOG=info", NULL};

    char *isolation[] = {"/vitrind", "--print-isolation", NULL};
    phase("isolation", isolation, envp);

    char *floor[] = {"/vitrind", "--print-floor", NULL};
    phase("floor", floor, envp);

    // The phase this whole harness exists for on an LTS kernel: does a default
    // session START on this machine, or does the isolation preflight refuse it?
    // `--consent=auto-approve` is chosen so an ADMITTED session exits promptly
    // instead of serving until the alarm; its exit status is therefore not the
    // verdict, and the collector reads the preflight's own line rather than the
    // status. See the page for why that distinction is load-bearing.
    char *startup[] = {"/vitrind", "--headless",       "--size",        "64x64",
                       "--consent", "auto-approve",    NULL};
    phase("startup", startup, envp);

    printf("VITRIN-INIT-DONE\n");
    fflush(stdout);
    sync();
    reboot(LINUX_REBOOT_CMD_POWER_OFF);
    for (;;) pause();
}
