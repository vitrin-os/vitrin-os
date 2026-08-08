// SPDX-License-Identifier: Apache-2.0
// probe-gbm-egl.c -- measure whether a usable GBM + EGL/GLES scanout path
// exists on a DRM device, for the ADVISORY `vkms-advisory` CI rung
// (WS-E.3.4, issue #220).
//
// Why this exists, and why it is a C program rather than `eglinfo`:
//
//   Issue #220 decision 5 says, in as many words, that whether a virtual KMS
//   device exercises the GBM + GLES scanout path is UNKNOWN until measured --
//   vkms exposes no render node, so the GLES half would need a software
//   renderer and may not import into a vkms scanout buffer at all. Writing an
//   acceptance criterion about it before measuring would be writing one that
//   may never legitimately go green. So this program measures, in stages, and
//   prints what it found; the caller turns that into a job summary.
//
//   `eglinfo` cannot answer it: it reports which EGL platforms a library
//   advertises, not whether a gbm surface made with GBM_BO_USE_SCANOUT on THIS
//   card can be rendered into and locked as a front buffer. That last step is
//   the one the DRM backend depends on and the one that plausibly fails on
//   vkms.
//
// What it deliberately does NOT do: it never calls drmSetMaster and never
// attempts a mode set or a page flip. It is a capability probe, not a
// bring-up; taking master on a CI runner would prove nothing extra and would
// make a read-only measurement into a stateful one. The mode-setting half of
// the answer is `drmModeGetResources`'s connector/CRTC counts, printed below.
//
// Exit status is ALWAYS 0 when the program ran to completion, including when
// stages fail. A failed stage is a measurement, not an error -- the caller is
// an advisory job that must never gate a PR. Non-zero means the probe itself
// could not run (bad usage).
//
// Output: one `stage=<name> result=<ok|fail|skip> detail=<...>` line per
// stage, on stdout, in execution order.

#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include <xf86drm.h>
#include <xf86drmMode.h>

#include <gbm.h>

#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>

static int failures = 0;

static void stage_ok(const char *name, const char *fmt, ...) {
    va_list ap;
    printf("stage=%s result=ok detail=", name);
    va_start(ap, fmt);
    vprintf(fmt, ap);
    va_end(ap);
    printf("\n");
    fflush(stdout);
}

static void stage_fail(const char *name, const char *fmt, ...) {
    va_list ap;
    failures++;
    printf("stage=%s result=fail detail=", name);
    va_start(ap, fmt);
    vprintf(fmt, ap);
    va_end(ap);
    printf("\n");
    fflush(stdout);
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s /dev/dri/cardN\n", argv[0]);
        return 2;
    }
    const char *node = argv[1];

    // ---- stage: open ----------------------------------------------------
    int fd = open(node, O_RDWR | O_CLOEXEC);
    if (fd < 0) {
        stage_fail("open", "%s: %s", node, strerror(errno));
        printf("summary=unusable reason=cannot-open-device\n");
        return 0;
    }
    drmVersionPtr ver = drmGetVersion(fd);
    stage_ok("open", "%s driver=%s", node, ver && ver->name ? ver->name : "?");

    // ---- stage: modeset-resources ---------------------------------------
    // Connector enumeration and CRTC count: the half of the DRM backend that
    // a virtual device is most likely to be able to exercise.
    drmModeRes *res = drmModeGetResources(fd);
    if (!res) {
        stage_fail("modeset-resources", "drmModeGetResources: %s",
                   strerror(errno));
    } else {
        int connected = 0;
        for (int i = 0; i < res->count_connectors; i++) {
            drmModeConnector *c = drmModeGetConnector(fd, res->connectors[i]);
            if (c) {
                if (c->connection == DRM_MODE_CONNECTED) connected++;
                drmModeFreeConnector(c);
            }
        }
        stage_ok("modeset-resources",
                 "connectors=%d connected=%d crtcs=%d encoders=%d",
                 res->count_connectors, connected, res->count_crtcs,
                 res->count_encoders);
        drmModeFreeResources(res);
    }

    // ---- stage: gbm-device ----------------------------------------------
    struct gbm_device *gbm = gbm_create_device(fd);
    if (!gbm) {
        stage_fail("gbm-device", "gbm_create_device returned NULL");
        printf("summary=no-gbm reason=gbm_create_device-failed\n");
        if (ver) drmFreeVersion(ver);
        close(fd);
        return 0;
    }
    stage_ok("gbm-device", "backend=%s", gbm_device_get_backend_name(gbm));

    // ---- stage: egl-display ---------------------------------------------
    EGLDisplay dpy = EGL_NO_DISPLAY;
    PFNEGLGETPLATFORMDISPLAYEXTPROC get_platform_display =
        (PFNEGLGETPLATFORMDISPLAYEXTPROC)eglGetProcAddress(
            "eglGetPlatformDisplayEXT");
    if (get_platform_display) {
        dpy = get_platform_display(EGL_PLATFORM_GBM_KHR, gbm, NULL);
    } else {
        dpy = eglGetDisplay((EGLNativeDisplayType)gbm);
    }
    EGLint major = 0, minor = 0;
    if (dpy == EGL_NO_DISPLAY || !eglInitialize(dpy, &major, &minor)) {
        stage_fail("egl-display", "eglInitialize failed (0x%04x)",
                   eglGetError());
        printf("summary=no-egl reason=eglInitialize-failed\n");
        gbm_device_destroy(gbm);
        if (ver) drmFreeVersion(ver);
        close(fd);
        return 0;
    }
    stage_ok("egl-display", "egl=%d.%d vendor=%s", major, minor,
             eglQueryString(dpy, EGL_VENDOR));

    // ---- stage: egl-config ----------------------------------------------
    static const EGLint cfg_attrs[] = {
        EGL_SURFACE_TYPE,    EGL_WINDOW_BIT,
        EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
        EGL_RED_SIZE,        8,
        EGL_GREEN_SIZE,      8,
        EGL_BLUE_SIZE,       8,
        EGL_NONE,
    };
    EGLConfig cfg;
    EGLint n = 0;
    if (!eglBindAPI(EGL_OPENGL_ES_API) ||
        !eglChooseConfig(dpy, cfg_attrs, &cfg, 1, &n) || n < 1) {
        stage_fail("egl-config", "no ES2 window config (0x%04x)",
                   eglGetError());
        printf("summary=no-gles-scanout reason=no-window-config\n");
        eglTerminate(dpy);
        gbm_device_destroy(gbm);
        if (ver) drmFreeVersion(ver);
        close(fd);
        return 0;
    }
    stage_ok("egl-config", "configs=%d", n);

    // ---- stage: gbm-scanout-surface -------------------------------------
    // GBM_BO_USE_SCANOUT is the load-bearing flag: this is the buffer a real
    // backend would hand to drmModePageFlip. A device with no scanout-capable
    // allocator fails HERE and nowhere earlier, which is exactly why the
    // probe goes this far.
    struct gbm_surface *surf =
        gbm_surface_create(gbm, 640, 480, GBM_FORMAT_XRGB8888,
                           GBM_BO_USE_SCANOUT | GBM_BO_USE_RENDERING);
    if (!surf) {
        stage_fail("gbm-scanout-surface",
                   "gbm_surface_create(SCANOUT|RENDERING, XRGB8888) failed");
        printf("summary=no-gles-scanout reason=no-scanout-surface\n");
        eglTerminate(dpy);
        gbm_device_destroy(gbm);
        if (ver) drmFreeVersion(ver);
        close(fd);
        return 0;
    }
    stage_ok("gbm-scanout-surface", "640x480 XRGB8888 SCANOUT|RENDERING");

    // ---- stage: gles-context --------------------------------------------
    static const EGLint ctx_attrs[] = {EGL_CONTEXT_CLIENT_VERSION, 2,
                                       EGL_NONE};
    EGLContext ctx = eglCreateContext(dpy, cfg, EGL_NO_CONTEXT, ctx_attrs);
    EGLSurface egl_surf = EGL_NO_SURFACE;
    if (ctx != EGL_NO_CONTEXT) {
        egl_surf = eglCreateWindowSurface(dpy, cfg,
                                          (EGLNativeWindowType)surf, NULL);
    }
    if (ctx == EGL_NO_CONTEXT || egl_surf == EGL_NO_SURFACE ||
        !eglMakeCurrent(dpy, egl_surf, egl_surf, ctx)) {
        stage_fail("gles-context", "make-current failed (0x%04x)",
                   eglGetError());
        printf("summary=no-gles-scanout reason=no-gles-context\n");
        gbm_surface_destroy(surf);
        eglTerminate(dpy);
        gbm_device_destroy(gbm);
        if (ver) drmFreeVersion(ver);
        close(fd);
        return 0;
    }
    stage_ok("gles-context", "renderer=%s", (const char *)glGetString(GL_RENDERER));

    // ---- stage: render-and-lock-front ------------------------------------
    // The whole question, in three calls: render, swap, and take the front
    // buffer the way a page flip would. This is the step vkms is most likely
    // to fail, and the reason the summary line below is worth reading.
    glClearColor(0.0f, 0.5f, 1.0f, 1.0f);
    glClear(GL_COLOR_BUFFER_BIT);
    if (!eglSwapBuffers(dpy, egl_surf)) {
        stage_fail("render-and-lock-front", "eglSwapBuffers failed (0x%04x)",
                   eglGetError());
    } else {
        struct gbm_bo *bo = gbm_surface_lock_front_buffer(surf);
        if (!bo) {
            stage_fail("render-and-lock-front",
                       "gbm_surface_lock_front_buffer returned NULL");
        } else {
            stage_ok("render-and-lock-front",
                     "bo stride=%u handle=%u modifier=0x%llx",
                     gbm_bo_get_stride(bo), gbm_bo_get_handle(bo).u32,
                     (unsigned long long)gbm_bo_get_modifier(bo));
            gbm_surface_release_buffer(surf, bo);
        }
    }

    printf("summary=%s reason=%s\n",
           failures == 0 ? "gbm-egl-scanout-usable" : "partial",
           failures == 0 ? "all-stages-passed" : "see-failed-stages-above");

    eglMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    eglDestroySurface(dpy, egl_surf);
    eglDestroyContext(dpy, ctx);
    gbm_surface_destroy(surf);
    eglTerminate(dpy);
    gbm_device_destroy(gbm);
    if (ver) drmFreeVersion(ver);
    close(fd);
    return 0;
}
