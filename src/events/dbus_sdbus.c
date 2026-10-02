#include <systemd/sd-bus.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <stdint.h>
#include <unistd.h>
#include <sys/eventfd.h>
#include <poll.h>

typedef void (*cellbar_dbus_callback)(uint64_t sub_id, const char *subscription, const char *context_json, void *userdata);

typedef struct cellbar_dbus_match {
    uint64_t sub_id;
    char *subscription;
    sd_bus_slot *slot;
    struct cellbar_dbus_context *ctx;
    struct cellbar_dbus_match *next;
} cellbar_dbus_match;

typedef struct cellbar_dbus_context {
    sd_bus *session_bus;
    sd_bus *system_bus;
    cellbar_dbus_callback callback;
    void *userdata;
    int wake_fd;
    cellbar_dbus_match *matches;
} cellbar_dbus_context;

static void json_escape(const char *in, char *out, size_t out_size) {
    if (!in) {
        snprintf(out, out_size, "\"\"");
        return;
    }
    size_t j = 0;
    out[j++] = '"';
    for (size_t i = 0; in[i] && j + 4 < out_size; i++) {
        switch (in[i]) {
            case '"': out[j++] = '\\'; out[j++] = '"'; break;
            case '\\': out[j++] = '\\'; out[j++] = '\\'; break;
            case '\n': out[j++] = '\\'; out[j++] = 'n'; break;
            case '\r': out[j++] = '\\'; out[j++] = 'r'; break;
            case '\t': out[j++] = '\\'; out[j++] = 't'; break;
            default:
                if ((unsigned char)in[i] < 32) {
                    j += snprintf(out + j, out_size - j, "\\u%04x", in[i]);
                } else {
                    out[j++] = in[i];
                }
                break;
        }
    }
    if (j < out_size - 1) out[j++] = '"';
    out[j] = '\0';
}

static int on_signal_message(sd_bus_message *m, void *userdata, sd_bus_error *ret_error) {
    (void)ret_error;
    cellbar_dbus_match *match = (cellbar_dbus_match *)userdata;
    if (!match || !match->ctx || !match->ctx->callback) return 0;

    const char *path = sd_bus_message_get_path(m);
    const char *iface = sd_bus_message_get_interface(m);
    const char *member = sd_bus_message_get_member(m);
    const char *sender = sd_bus_message_get_sender(m);
    const char *destination = sd_bus_message_get_destination(m);

    char esc_sub[256], esc_path[512], esc_iface[256], esc_member[256], esc_sender[128], esc_dest[128];
    json_escape(match->subscription, esc_sub, sizeof(esc_sub));
    json_escape(path, esc_path, sizeof(esc_path));
    json_escape(iface, esc_iface, sizeof(esc_iface));
    json_escape(member, esc_member, sizeof(esc_member));
    json_escape(sender, esc_sender, sizeof(esc_sender));
    json_escape(destination, esc_dest, sizeof(esc_dest));

    char buf[4096];
    snprintf(buf, sizeof(buf),
        "{\"version\":1,\"subscription\":%s,\"message\":{\"type\":\"signal\",\"sender\":%s,\"destination\":%s,\"path\":%s,\"interface\":%s,\"member\":%s,\"body\":null}}",
        esc_sub, esc_sender, esc_dest, esc_path, esc_iface, esc_member);

    match->ctx->callback(match->sub_id, match->subscription, buf, match->ctx->userdata);
    return 0;
}

cellbar_dbus_context *cellbar_dbus_new(cellbar_dbus_callback callback, void *userdata) {
    cellbar_dbus_context *ctx = (cellbar_dbus_context *)calloc(1, sizeof(cellbar_dbus_context));
    if (!ctx) return NULL;

    ctx->callback = callback;
    ctx->userdata = userdata;
    ctx->wake_fd = eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC);

    // Try connecting to session bus
    sd_bus_open_user(&ctx->session_bus);

    return ctx;
}

void cellbar_dbus_wake(cellbar_dbus_context *ctx) {
    if (ctx && ctx->wake_fd >= 0) {
        uint64_t val = 1;
        write(ctx->wake_fd, &val, sizeof(val));
    }
}

cellbar_dbus_match *cellbar_dbus_add_match(
    cellbar_dbus_context *ctx,
    int is_system,
    uint64_t sub_id,
    const char *subscription,
    const char *match_rule
) {
    if (!ctx) return NULL;

    sd_bus **bus_ptr = is_system ? &ctx->system_bus : &ctx->session_bus;
    if (!*bus_ptr) {
        if (is_system) {
            sd_bus_open_system(bus_ptr);
        } else {
            sd_bus_open_user(bus_ptr);
        }
    }
    if (!*bus_ptr) return NULL;

    cellbar_dbus_match *m = (cellbar_dbus_match *)calloc(1, sizeof(cellbar_dbus_match));
    if (!m) return NULL;

    m->sub_id = sub_id;
    m->subscription = strdup(subscription ? subscription : "");
    m->ctx = ctx;

    int r = sd_bus_add_match(*bus_ptr, &m->slot, match_rule, on_signal_message, m);
    if (r < 0) {
        free(m->subscription);
        free(m);
        return NULL;
    }

    m->next = ctx->matches;
    ctx->matches = m;
    cellbar_dbus_wake(ctx);
    return m;
}

void cellbar_dbus_remove_match(cellbar_dbus_context *ctx, uint64_t sub_id) {
    if (!ctx) return;

    cellbar_dbus_match **curr = &ctx->matches;
    while (*curr) {
        if ((*curr)->sub_id == sub_id) {
            cellbar_dbus_match *target = *curr;
            *curr = target->next;
            if (target->slot) sd_bus_slot_unref(target->slot);
            free(target->subscription);
            free(target);
            cellbar_dbus_wake(ctx);
            return;
        }
        curr = &(*curr)->next;
    }
}

int cellbar_dbus_step(cellbar_dbus_context *ctx, int timeout_ms) {
    if (!ctx) return -1;

    int r;
    if (ctx->session_bus) {
        while ((r = sd_bus_process(ctx->session_bus, NULL)) > 0);
    }
    if (ctx->system_bus) {
        while ((r = sd_bus_process(ctx->system_bus, NULL)) > 0);
    }

    struct pollfd fds[3];
    int nfds = 0;

    fds[nfds].fd = ctx->wake_fd;
    fds[nfds].events = POLLIN;
    fds[nfds].revents = 0;
    nfds++;

    if (ctx->session_bus) {
        fds[nfds].fd = sd_bus_get_fd(ctx->session_bus);
        fds[nfds].events = POLLIN;
        fds[nfds].revents = 0;
        nfds++;
    }
    if (ctx->system_bus) {
        fds[nfds].fd = sd_bus_get_fd(ctx->system_bus);
        fds[nfds].events = POLLIN;
        fds[nfds].revents = 0;
        nfds++;
    }

    poll(fds, nfds, timeout_ms);

    if (fds[0].revents & POLLIN) {
        uint64_t val;
        read(ctx->wake_fd, &val, sizeof(val));
    }

    if (ctx->session_bus) {
        while ((r = sd_bus_process(ctx->session_bus, NULL)) > 0);
    }
    if (ctx->system_bus) {
        while ((r = sd_bus_process(ctx->system_bus, NULL)) > 0);
    }

    return 0;
}

void cellbar_dbus_free(cellbar_dbus_context *ctx) {
    if (!ctx) return;

    cellbar_dbus_match *m = ctx->matches;
    while (m) {
        cellbar_dbus_match *next = m->next;
        if (m->slot) sd_bus_slot_unref(m->slot);
        free(m->subscription);
        free(m);
        m = next;
    }

    if (ctx->session_bus) {
        sd_bus_flush_close_unref(ctx->session_bus);
    }
    if (ctx->system_bus) {
        sd_bus_flush_close_unref(ctx->system_bus);
    }
    if (ctx->wake_fd >= 0) {
        close(ctx->wake_fd);
    }
    free(ctx);
}
