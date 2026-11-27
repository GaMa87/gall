/* Presentation feedback on GTK's own Wayland connection. Calls and callbacks
 * run on GTK's thread. Each outstanding feedback owns its callback data. */
#include <gdk/gdkwayland.h>
#include <wayland-client.h>

#include "presentation-client.h"

static struct wp_presentation *presentation;

static void clock_id(void *data, struct wp_presentation *object, uint32_t id) {}
static const struct wp_presentation_listener presentation_listener = {clock_id};

static void global(void *data, struct wl_registry *registry, uint32_t name,
                   const char *interface, uint32_t version) {
    if (!presentation && g_str_equal(interface, "wp_presentation")) {
        presentation = wl_registry_bind(registry, name, &wp_presentation_interface, 1);
        wp_presentation_add_listener(presentation, &presentation_listener, NULL);
    }
}

static void global_remove(void *data, struct wl_registry *registry, uint32_t name) {}
static const struct wl_registry_listener registry_listener = {global, global_remove};

int install_feedback(GdkDisplay *display) {
    if (!GDK_IS_WAYLAND_DISPLAY(display)) return 0;

    struct wl_display *wayland = gdk_wayland_display_get_wl_display(display);
    struct wl_registry *registry = wl_display_get_registry(wayland);
    wl_registry_add_listener(registry, &registry_listener, NULL);
    int result = wl_display_roundtrip(wayland);
    wl_registry_destroy(registry);
    return result >= 0 && presentation != NULL;
}

typedef void (*Notify)(uint64_t timestamp_ns, uint32_t refresh_ns, uint32_t flags,
                       void *userdata);
typedef struct {
    struct wp_presentation_feedback *feedback;
    guint timeout;
    Notify notify;
    void *userdata;
} PendingFrame;

static void complete(PendingFrame *frame, uint64_t timestamp, uint32_t refresh,
                     uint32_t flags) {
    wp_presentation_feedback_destroy(frame->feedback);
    if (frame->timeout) g_source_remove(frame->timeout);
    frame->notify(timestamp, refresh, flags, frame->userdata);
    g_free(frame);
}

static void sync_output(void *data, struct wp_presentation_feedback *feedback,
                        struct wl_output *output) {}

static void presented(void *data, struct wp_presentation_feedback *feedback,
                      uint32_t hi, uint32_t lo, uint32_t ns, uint32_t refresh,
                      uint32_t seqhi, uint32_t seqlo, uint32_t flags) {
    uint64_t seconds = ((uint64_t) hi << 32) | lo;
    complete(data, seconds * G_GUINT64_CONSTANT(1000000000) + ns, refresh, flags);
}

static void discarded(void *data, struct wp_presentation_feedback *feedback) {
    complete(data, 0, 0, 0);
}

static gboolean timed_out(gpointer data) {
    PendingFrame *frame = data;
    frame->timeout = 0;
    complete(frame, 0, 0, 0);
    return G_SOURCE_REMOVE;
}

static const struct wp_presentation_feedback_listener feedback_listener = {
    sync_output, presented, discarded
};

int request_feedback(GdkWindow *window, Notify notify, void *userdata) {
    if (!presentation || !GDK_IS_WAYLAND_WINDOW(window)) return 0;
    struct wl_surface *surface = gdk_wayland_window_get_wl_surface(window);
    if (!surface) return 0;

    PendingFrame *frame = g_new(PendingFrame, 1);
    frame->notify = notify;
    frame->userdata = userdata;
    frame->feedback = wp_presentation_feedback(presentation, surface);
    wp_presentation_feedback_add_listener(frame->feedback, &feedback_listener, frame);
    frame->timeout = g_timeout_add(750, timed_out, frame);
    return 1;
}

/* Debug self-tests use these timestamps to time real output refreshes. The
 * browser clock has millisecond granularity and can report 17 ms at 60 Hz. */
#define TRACE_KEY "gall-presentation-trace"
#define TRACE_CAPACITY 256

typedef struct {
    uint64_t timestamp;
    uint64_t refresh;
} FrameSample;

typedef struct {
    grefcount refs;
    gulong paint_handler;
    size_t count;
    FrameSample samples[TRACE_CAPACITY];
} FrameTrace;

static void release_trace(void *data) {
    FrameTrace *trace = data;
    if (g_ref_count_dec(&trace->refs)) g_free(trace);
}

static void record_sample(uint64_t timestamp, uint32_t refresh, uint32_t flags, void *data) {
    FrameTrace *trace = data;
    const uint32_t hardware = WP_PRESENTATION_FEEDBACK_KIND_VSYNC | WP_PRESENTATION_FEEDBACK_KIND_HW_CLOCK;
    if ((flags & hardware) == hardware && timestamp && trace->count < TRACE_CAPACITY) {
        trace->samples[trace->count++] = (FrameSample){timestamp, refresh};
    }
    release_trace(trace);
}

static void trace_paint(GdkFrameClock *clock, GdkWindow *window) {
    FrameTrace *trace = g_object_get_data(G_OBJECT(window), TRACE_KEY);
    if (!trace || gdk_window_is_destroyed(window) || trace->count >= TRACE_CAPACITY) return;
    g_ref_count_inc(&trace->refs);
    if (!request_feedback(window, record_sample, trace)) release_trace(trace);
}

int start_frame_trace(GdkWindow *window) {
    if (!presentation || !GDK_IS_WAYLAND_WINDOW(window)) return 0;
    GdkFrameClock *clock = gdk_window_get_frame_clock(window);
    if (!clock) return 0;
    if (g_object_get_data(G_OBJECT(window), TRACE_KEY)) return 1;
    FrameTrace *trace = g_new0(FrameTrace, 1);
    g_ref_count_init(&trace->refs);
    trace->paint_handler = g_signal_connect_object(clock, "before-paint", G_CALLBACK(trace_paint), window, 0);
    g_object_set_data_full(G_OBJECT(window), TRACE_KEY, trace, release_trace);
    return 1;
}

size_t stop_frame_trace(GdkWindow *window, FrameSample *samples, size_t capacity) {
    FrameTrace *trace = g_object_steal_data(G_OBJECT(window), TRACE_KEY);
    if (!trace) return 0;
    GdkFrameClock *clock = gdk_window_get_frame_clock(window);
    if (clock) g_signal_handler_disconnect(clock, trace->paint_handler);
    size_t count = MIN(capacity, trace->count);
    for (size_t i = 0; i < count; i++) samples[i] = trace->samples[i];
    release_trace(trace);
    return count;
}
