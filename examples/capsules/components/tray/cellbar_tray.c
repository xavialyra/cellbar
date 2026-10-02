#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <signal.h>
#include <unistd.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <dirent.h>
#include <arpa/inet.h>
#include <zlib.h>
#include <systemd/sd-bus.h>

#define MAX_ITEMS 64
#define PATH_BUF_SIZE 1024

static char runtime_dir[PATH_BUF_SIZE];
static char generated_dir[PATH_BUF_SIZE];
static char state_file[PATH_BUF_SIZE];

static char *items[MAX_ITEMS];
static size_t item_count = 0;
static sd_bus *bus = NULL;
static volatile sig_atomic_t running = 1;

static const char *opt_icon_mode = "image";
static int opt_icon_width = 2;
static const char *opt_icon_fit = "contain";
static const char *opt_spacing = "[ ]";
static int opt_hide_passive = 1;

static void update_and_render(void);

static void handle_sig(int sig) {
    (void)sig;
    running = 0;
}

static void init_paths(void) {
    const char *xdg = getenv("XDG_RUNTIME_DIR");
    char base_dir[PATH_BUF_SIZE];
    if (xdg && xdg[0]) {
        snprintf(base_dir, sizeof(base_dir), "%s/cellbar", xdg);
    } else {
        snprintf(base_dir, sizeof(base_dir), "/tmp/cellbar-%d", getuid());
    }
    mkdir(base_dir, 0700);
    snprintf(runtime_dir, sizeof(runtime_dir), "%s/tray", base_dir);
    mkdir(runtime_dir, 0700);

    snprintf(generated_dir, sizeof(generated_dir), "%s/icons", runtime_dir);
    snprintf(state_file, sizeof(state_file), "%s/items.json", runtime_dir);

    mkdir(generated_dir, 0700);
}

static void write_png_chunk(FILE *f, const char *type, const uint8_t *data, uint32_t len) {
    uint32_t len_be = htonl(len);
    fwrite(&len_be, 1, 4, f);
    fwrite(type, 1, 4, f);
    if (len > 0 && data) fwrite(data, 1, len, f);
    uint32_t crc = crc32(0L, Z_NULL, 0);
    crc = crc32(crc, (const Bytef *)type, 4);
    if (len > 0 && data) crc = crc32(crc, (const Bytef *)data, len);
    uint32_t crc_be = htonl(crc);
    fwrite(&crc_be, 1, 4, f);
}

static int save_argb_pixmap_png(int width, int height, const uint8_t *argb, const char *out_path) {
    if (width <= 0 || height <= 0 || !argb) return -1;

    size_t raw_row_bytes = width * 4;
    size_t raw_len = (raw_row_bytes + 1) * height;
    uint8_t *raw = malloc(raw_len);
    if (!raw) return -1;

    size_t out_idx = 0;
    for (int y = 0; y < height; y++) {
        raw[out_idx++] = 0; // Filter: None
        for (int x = 0; x < width; x++) {
            size_t in_idx = (y * width + x) * 4;
            uint8_t a = argb[in_idx + 0];
            uint8_t r = argb[in_idx + 1];
            uint8_t g = argb[in_idx + 2];
            uint8_t b = argb[in_idx + 3];
            raw[out_idx++] = r;
            raw[out_idx++] = g;
            raw[out_idx++] = b;
            raw[out_idx++] = a;
        }
    }

    uLongf comp_len = compressBound(raw_len);
    uint8_t *comp = malloc(comp_len);
    if (!comp) { free(raw); return -1; }
    if (compress(comp, &comp_len, raw, raw_len) != Z_OK) {
        free(raw); free(comp); return -1;
    }
    free(raw);

    char tmp_path[PATH_BUF_SIZE];
    snprintf(tmp_path, sizeof(tmp_path), "%s.tmp.%d", out_path, getpid());
    FILE *f = fopen(tmp_path, "wb");
    if (!f) { free(comp); return -1; }

    static const uint8_t png_header[8] = { 0x89, 'P', 'N', 'G', 0x0D, 0x0A, 0x1A, 0x0A };
    fwrite(png_header, 1, 8, f);

    uint8_t ihdr[13];
    uint32_t w_be = htonl(width);
    uint32_t h_be = htonl(height);
    memcpy(ihdr + 0, &w_be, 4);
    memcpy(ihdr + 4, &h_be, 4);
    ihdr[8] = 8;
    ihdr[9] = 6;
    ihdr[10] = 0;
    ihdr[11] = 0;
    ihdr[12] = 0;
    write_png_chunk(f, "IHDR", ihdr, 13);
    write_png_chunk(f, "IDAT", comp, comp_len);
    free(comp);
    write_png_chunk(f, "IEND", NULL, 0);
    fclose(f);

    rename(tmp_path, out_path);
    return 0;
}

static int file_exists(const char *path) {
    return access(path, F_OK) == 0;
}

static int convert_svg_to_png(const char *svg_path, const char *out_png) {
    char tmp_png[PATH_BUF_SIZE];
    snprintf(tmp_png, sizeof(tmp_png), "%s.tmp.%d", out_png, getpid());

    char cmd[PATH_BUF_SIZE * 2];
    snprintf(cmd, sizeof(cmd), "rsvg-convert -w 32 -h 32 '%s' -o '%s' 2>/dev/null", svg_path, tmp_png);
    if (system(cmd) == 0 && file_exists(tmp_png)) {
        rename(tmp_png, out_png);
        return 0;
    }

    snprintf(cmd, sizeof(cmd), "magick -background none '%s' -resize 32x32 '%s' 2>/dev/null", svg_path, tmp_png);
    if (system(cmd) == 0 && file_exists(tmp_png)) {
        rename(tmp_png, out_png);
        return 0;
    }

    unlink(tmp_png);
    return -1;
}

static int search_dir_for_file(const char *dir, const char *target, char *out, size_t out_sz, int depth) {
    if (depth > 6) return 0;
    DIR *d = opendir(dir);
    if (!d) return 0;
    struct dirent *ent;
    int found = 0;
    while ((ent = readdir(d)) != NULL) {
        if (ent->d_name[0] == '.') continue;
        char sub[PATH_BUF_SIZE];
        snprintf(sub, sizeof(sub), "%s/%s", dir, ent->d_name);
        struct stat st;
        if (stat(sub, &st) < 0) continue;
        if (S_ISDIR(st.st_mode)) {
            if (search_dir_for_file(sub, target, out, out_sz, depth + 1)) {
                found = 1;
                break;
            }
        } else if (strcmp(ent->d_name, target) == 0) {
            snprintf(out, out_sz, "%s", sub);
            found = 1;
            break;
        }
    }
    closedir(d);
    return found;
}

static int resolve_icon_path(const char *icon_name, const char *custom_theme_path, char *out_path, size_t out_sz) {
    if (!icon_name || !icon_name[0]) return 0;

    char safe_name[PATH_BUF_SIZE];
    size_t j = 0;
    for (const char *p = icon_name; *p && j + 1 < sizeof(safe_name); p++) {
        char c = *p;
        if ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '-' || c == '_') {
            safe_name[j++] = c;
        } else {
            safe_name[j++] = '_';
        }
    }
    safe_name[j] = '\0';

    char rendered[PATH_BUF_SIZE];
    snprintf(rendered, sizeof(rendered), "%s/svg_%s.png", generated_dir, safe_name);
    if (file_exists(rendered)) {
        snprintf(out_path, out_sz, "%s", rendered);
        return 1;
    }

    if (icon_name[0] == '/' && file_exists(icon_name)) {
        size_t len = strlen(icon_name);
        if (len > 4 && (strcasecmp(icon_name + len - 4, ".png") == 0 ||
                        strcasecmp(icon_name + len - 4, ".jpg") == 0 ||
                        strcasecmp(icon_name + len - 5, ".jpeg") == 0 ||
                        strcasecmp(icon_name + len - 5, ".webp") == 0)) {
            snprintf(out_path, out_sz, "%s", icon_name);
            return 1;
        }
        if (len > 4 && strcasecmp(icon_name + len - 4, ".svg") == 0) {
            if (convert_svg_to_png(icon_name, rendered) == 0) {
                snprintf(out_path, out_sz, "%s", rendered);
                return 1;
            }
        }
    }

    char base_name[256];
    snprintf(base_name, sizeof(base_name), "%s", icon_name);
    char *dot = strrchr(base_name, '.');
    if (dot && (strcmp(dot, ".png") == 0 || strcmp(dot, ".svg") == 0 || strcmp(dot, ".xpm") == 0)) {
        *dot = '\0';
    }

    char png_target[300], svg_target[300];
    snprintf(png_target, sizeof(png_target), "%s.png", base_name);
    snprintf(svg_target, sizeof(svg_target), "%s.svg", base_name);

    const char *home = getenv("HOME");
    const char *xdg_data_home = getenv("XDG_DATA_HOME");
    const char *xdg_data_dirs = getenv("XDG_DATA_DIRS");
    if (!xdg_data_dirs || !xdg_data_dirs[0]) {
        xdg_data_dirs = "/usr/local/share:/usr/share";
    }

    char search_dirs[20][PATH_BUF_SIZE];
    int n_dirs = 0;

    if (custom_theme_path && custom_theme_path[0] && n_dirs < 20) {
        snprintf(search_dirs[n_dirs++], PATH_BUF_SIZE, "%s", custom_theme_path);
    }
    if (xdg_data_home && xdg_data_home[0] && n_dirs < 20) {
        snprintf(search_dirs[n_dirs++], PATH_BUF_SIZE, "%s/icons", xdg_data_home);
        snprintf(search_dirs[n_dirs++], PATH_BUF_SIZE, "%s/pixmaps", xdg_data_home);
    } else if (home && home[0] && n_dirs + 1 < 20) {
        snprintf(search_dirs[n_dirs++], PATH_BUF_SIZE, "%s/.local/share/icons", home);
        snprintf(search_dirs[n_dirs++], PATH_BUF_SIZE, "%s/.local/share/pixmaps", home);
    }
    if (home && home[0] && n_dirs < 20) {
        snprintf(search_dirs[n_dirs++], PATH_BUF_SIZE, "%s/.icons", home);
    }

    char dirs_copy[PATH_BUF_SIZE * 2];
    snprintf(dirs_copy, sizeof(dirs_copy), "%s", xdg_data_dirs);
    char *saveptr = NULL;
    for (char *tok = strtok_r(dirs_copy, ":", &saveptr); tok && n_dirs + 1 < 20; tok = strtok_r(NULL, ":", &saveptr)) {
        if (!tok[0]) continue;
        snprintf(search_dirs[n_dirs++], PATH_BUF_SIZE, "%s/icons", tok);
        snprintf(search_dirs[n_dirs++], PATH_BUF_SIZE, "%s/pixmaps", tok);
    }

    char found_png[PATH_BUF_SIZE] = {0};
    char found_svg[PATH_BUF_SIZE] = {0};

    for (int i = 0; i < n_dirs; i++) {
        if (!file_exists(search_dirs[i])) continue;

        if (!found_png[0] && search_dir_for_file(search_dirs[i], png_target, found_png, sizeof(found_png), 0)) {
            snprintf(out_path, out_sz, "%s", found_png);
            return 1;
        }
        if (!found_svg[0]) {
            search_dir_for_file(search_dirs[i], svg_target, found_svg, sizeof(found_svg), 0);
        }
    }

    if (found_png[0]) {
        snprintf(out_path, out_sz, "%s", found_png);
        return 1;
    }

    if (found_svg[0]) {
        if (convert_svg_to_png(found_svg, rendered) == 0) {
            snprintf(out_path, out_sz, "%s", rendered);
            return 1;
        }
    }

    return 0;
}

static const char *get_fallback_glyph(const char *category, const char *title, const char *id) {
    if (category) {
        if (strstr(category, "Communications")) return "󰭹";
        if (strstr(category, "SystemServices")) return "󱊖";
        if (strstr(category, "Hardware")) return "󰒋";
    }
    if (title) {
        if (strcasestr(title, "volume") || strcasestr(title, "audio")) return "";
        if (strcasestr(title, "network") || strcasestr(title, "wifi")) return "󰖩";
        if (strcasestr(title, "bluetooth")) return "󰂯";
    }
    if (id) {
        if (strcasestr(id, "fcitx") || strcasestr(id, "input")) return "󰌌";
    }
    return "󰐍";
}

static int read_pixmap_variant(sd_bus_message *reply, char *out_png, size_t out_sz, const char *service) {
    int r = sd_bus_message_enter_container(reply, 'v', "a(iiay)");
    if (r < 0) return 0;
    r = sd_bus_message_enter_container(reply, 'a', "(iiay)");
    if (r < 0) {
        sd_bus_message_exit_container(reply);
        return 0;
    }

    int best_w = 0, best_h = 0;
    const void *best_data = NULL;

    while ((r = sd_bus_message_enter_container(reply, 'r', "iiay")) > 0) {
        int32_t w = 0, h = 0;
        sd_bus_message_read(reply, "ii", &w, &h);
        const void *data = NULL;
        size_t sz = 0;
        sd_bus_message_read_array(reply, 'y', &data, &sz);
        if (w >= 16 && sz >= (size_t)(w * h * 4)) {
            if (best_w == 0 || (best_w < 20 && w >= 20) || (w <= 64 && w > best_w)) {
                best_w = w;
                best_h = h;
                best_data = data;
            }
        }
        sd_bus_message_exit_container(reply);
    }

    sd_bus_message_exit_container(reply);
    sd_bus_message_exit_container(reply);

    if (best_w > 0 && best_data) {
        char safe_svc[128];
        size_t j = 0;
        for (const char *p = service; *p && j + 1 < sizeof(safe_svc); p++) {
            char c = *p;
            safe_svc[j++] = ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9')) ? c : '_';
        }
        safe_svc[j] = '\0';

        snprintf(out_png, out_sz, "%s/pixmap_%s_%dx%d.png", generated_dir, safe_svc, best_w, best_h);
        if (file_exists(out_png) || save_argb_pixmap_png(best_w, best_h, (const uint8_t *)best_data, out_png) == 0) {
            return 1;
        }
    }
    return 0;
}

static void update_and_render(void) {
    if (!bus) return;

    char markup_output[8192] = {0};
    char json_mapping[8192] = {0};
    strcat(json_mapping, "{\n");

    int valid_idx = 0;

    for (size_t i = 0; i < item_count; i++) {
        const char *item = items[i];
        if (!item || !item[0]) continue;

        char service[256] = {0};
        char path[256] = {0};
        const char *slash = strchr(item, '/');
        if (slash) {
            size_t s_len = slash - item;
            if (s_len >= sizeof(service)) s_len = sizeof(service) - 1;
            strncpy(service, item, s_len);
            snprintf(path, sizeof(path), "%s", slash);
        } else {
            snprintf(service, sizeof(service), "%s", item);
            snprintf(path, sizeof(path), "/StatusNotifierItem");
        }

        char *item_id = NULL, *title = NULL, *status = NULL, *category = NULL;
        char *icon_name = NULL, *attention_icon = NULL, *custom_theme_path = NULL, *ayatana_label = NULL;

        sd_bus_get_property_string(bus, service, path, "org.kde.StatusNotifierItem", "Id", NULL, &item_id);
        sd_bus_get_property_string(bus, service, path, "org.kde.StatusNotifierItem", "Title", NULL, &title);
        sd_bus_get_property_string(bus, service, path, "org.kde.StatusNotifierItem", "Status", NULL, &status);
        sd_bus_get_property_string(bus, service, path, "org.kde.StatusNotifierItem", "Category", NULL, &category);
        sd_bus_get_property_string(bus, service, path, "org.kde.StatusNotifierItem", "IconName", NULL, &icon_name);
        sd_bus_get_property_string(bus, service, path, "org.kde.StatusNotifierItem", "AttentionIconName", NULL, &attention_icon);
        sd_bus_get_property_string(bus, service, path, "org.kde.StatusNotifierItem", "IconThemePath", NULL, &custom_theme_path);
        sd_bus_get_property_string(bus, service, path, "org.kde.StatusNotifierItem", "XAyatanaLabel", NULL, &ayatana_label);

        if (opt_hide_passive && status && strcmp(status, "Passive") == 0) {
            free(item_id); free(title); free(status); free(category);
            free(icon_name); free(attention_icon); free(custom_theme_path); free(ayatana_label);
            continue;
        }

        char resolved_img[PATH_BUF_SIZE] = {0};
        int is_attention = (status && strcmp(status, "NeedsAttention") == 0);
        const char *active_icon_name = (is_attention && attention_icon && attention_icon[0]) ? attention_icon : icon_name;

        if (strcmp(opt_icon_mode, "font") != 0) {
            const char *pix_prop = is_attention ? "AttentionIconPixmap" : "IconPixmap";
            sd_bus_message *reply = NULL;
            int pr = sd_bus_call_method(bus, service, path, "org.freedesktop.DBus.Properties", "Get", NULL, &reply,
                                        "ss", "org.kde.StatusNotifierItem", pix_prop);
            if (pr >= 0) {
                read_pixmap_variant(reply, resolved_img, sizeof(resolved_img), service);
                sd_bus_message_unref(reply);
            }
            if (!resolved_img[0] && active_icon_name && active_icon_name[0]) {
                resolve_icon_path(active_icon_name, custom_theme_path, resolved_img, sizeof(resolved_img));
            }
        }

        const char *style = is_attention ? "@warning" : "@tray";
        const char *fallback_char = get_fallback_glyph(category, title, item_id);

        char item_rendered[512] = {0};
        if (ayatana_label && ayatana_label[0]) {
            snprintf(item_rendered, sizeof(item_rendered), "[%s](%s)", ayatana_label, style);
        } else if (resolved_img[0]) {
            snprintf(item_rendered, sizeof(item_rendered), "![%s](%d %s fallback=%s)",
                     resolved_img, opt_icon_width, opt_icon_fit, fallback_char);
        } else {
            snprintf(item_rendered, sizeof(item_rendered), "[%s](%s)", fallback_char, style);
        }

        char item_markup[600];
        snprintf(item_markup, sizeof(item_markup), "#tray:%d{ %s }", valid_idx, item_rendered);

        if (valid_idx > 0) {
            if (opt_spacing[0] == '[') {
                strcat(markup_output, opt_spacing);
            } else if (opt_spacing[0] != '\0') {
                strcat(markup_output, "[");
                strcat(markup_output, opt_spacing);
                strcat(markup_output, "]");
            }
            strcat(json_mapping, ",\n");
        }
        strcat(markup_output, item_markup);

        char entry_json[512];
        snprintf(entry_json, sizeof(entry_json),
                 "  \"%d\": {\"service\": \"%s\", \"path\": \"%s\", \"id\": \"%s\", \"title\": \"%s\"}",
                 valid_idx, service, path, item_id ? item_id : "", title ? title : "");
        strcat(json_mapping, entry_json);

        valid_idx++;

        free(item_id); free(title); free(status); free(category);
        free(icon_name); free(attention_icon); free(custom_theme_path); free(ayatana_label);
    }

    strcat(json_mapping, "\n}\n");

    // Atomically write state file
    if (valid_idx > 0) {
        char tmp_state[PATH_BUF_SIZE];
        snprintf(tmp_state, sizeof(tmp_state), "%s.tmp.%d", state_file, getpid());
        FILE *sf = fopen(tmp_state, "w");
        if (sf) {
            fputs(json_mapping, sf);
            fclose(sf);
            rename(tmp_state, state_file);
        }
    } else {
        unlink(state_file);
    }

    // Stream single line of markup to Cellbar over stdout
    puts(markup_output);
    fflush(stdout);
}

static int find_item(const char *item) {
    for (size_t i = 0; i < item_count; i++) {
        if (strcmp(items[i], item) == 0) return (int)i;
    }
    return -1;
}

static int on_item_signal(sd_bus_message *m, void *userdata, sd_bus_error *ret_error) {
    (void)m; (void)userdata; (void)ret_error;
    update_and_render();
    return 0;
}

static void subscribe_item_signals(const char *item) {
    char service[256] = {0};
    char path[256] = {0};
    const char *slash = strchr(item, '/');
    if (slash) {
        size_t s_len = slash - item;
        if (s_len >= sizeof(service)) s_len = sizeof(service) - 1;
        strncpy(service, item, s_len);
        snprintf(path, sizeof(path), "%s", slash);
    } else {
        snprintf(service, sizeof(service), "%s", item);
        snprintf(path, sizeof(path), "/StatusNotifierItem");
    }

    sd_bus_match_signal(bus, NULL, service, path, "org.kde.StatusNotifierItem", "NewIcon", on_item_signal, NULL);
    sd_bus_match_signal(bus, NULL, service, path, "org.kde.StatusNotifierItem", "NewAttentionIcon", on_item_signal, NULL);
    sd_bus_match_signal(bus, NULL, service, path, "org.kde.StatusNotifierItem", "NewStatus", on_item_signal, NULL);
    sd_bus_match_signal(bus, NULL, service, path, "org.kde.StatusNotifierItem", "NewTitle", on_item_signal, NULL);
    sd_bus_match_signal(bus, NULL, service, path, "org.freedesktop.DBus.Properties", "PropertiesChanged", on_item_signal, NULL);
}

static void add_item(const char *item) {
    if (!item || item[0] == '\0') return;
    if (find_item(item) >= 0) return;
    if (item_count < MAX_ITEMS) {
        items[item_count++] = strdup(item);
        subscribe_item_signals(item);
        sd_bus_emit_signal(bus, "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher",
                           "StatusNotifierItemRegistered", "s", item);
        update_and_render();
    }
}

static void remove_item(size_t idx) {
    if (idx >= item_count) return;
    char *item = items[idx];
    sd_bus_emit_signal(bus, "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher",
                       "StatusNotifierItemUnregistered", "s", item);
    free(item);
    for (size_t i = idx; i + 1 < item_count; i++) {
        items[i] = items[i + 1];
    }
    item_count--;
    update_and_render();
}

static int method_register_item(sd_bus_message *m, void *userdata, sd_bus_error *ret_error) {
    (void)userdata; (void)ret_error;
    const char *service = NULL;
    int r = sd_bus_message_read(m, "s", &service);
    if (r < 0 || !service) return r;

    char full[512];
    if (service[0] == '/') {
        const char *sender = sd_bus_message_get_sender(m);
        snprintf(full, sizeof(full), "%s%s", sender ? sender : "", service);
    } else if (strchr(service, '/') == NULL) {
        snprintf(full, sizeof(full), "%s/StatusNotifierItem", service);
    } else {
        snprintf(full, sizeof(full), "%s", service);
    }

    add_item(full);
    return sd_bus_reply_method_return(m, "");
}

static int method_register_host(sd_bus_message *m, void *userdata, sd_bus_error *ret_error) {
    (void)userdata; (void)ret_error;
    sd_bus_emit_signal(bus, "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher",
                       "StatusNotifierHostRegistered", "");
    return sd_bus_reply_method_return(m, "");
}

static int prop_get_items(sd_bus *b, const char *path, const char *interface,
                          const char *property, sd_bus_message *reply,
                          void *userdata, sd_bus_error *ret_error) {
    (void)b; (void)path; (void)interface; (void)property; (void)userdata; (void)ret_error;
    int r = sd_bus_message_open_container(reply, 'a', "s");
    if (r < 0) return r;
    for (size_t i = 0; i < item_count; i++) {
        r = sd_bus_message_append(reply, "s", items[i]);
        if (r < 0) return r;
    }
    return sd_bus_message_close_container(reply);
}

static int prop_get_host_reg(sd_bus *b, const char *path, const char *interface,
                             const char *property, sd_bus_message *reply,
                             void *userdata, sd_bus_error *ret_error) {
    (void)b; (void)path; (void)interface; (void)property; (void)userdata; (void)ret_error;
    int bval = 1;
    return sd_bus_message_append(reply, "b", bval);
}

static int prop_get_proto_ver(sd_bus *b, const char *path, const char *interface,
                              const char *property, sd_bus_message *reply,
                              void *userdata, sd_bus_error *ret_error) {
    (void)b; (void)path; (void)interface; (void)property; (void)userdata; (void)ret_error;
    int v = 0;
    return sd_bus_message_append(reply, "i", v);
}

static const sd_bus_vtable watcher_vtable[] = {
    SD_BUS_VTABLE_START(0),
    SD_BUS_METHOD("RegisterStatusNotifierItem", "s", "", method_register_item, SD_BUS_VTABLE_UNPRIVILEGED),
    SD_BUS_METHOD("RegisterStatusNotifierHost", "s", "", method_register_host, SD_BUS_VTABLE_UNPRIVILEGED),
    SD_BUS_SIGNAL("StatusNotifierItemRegistered", "s", 0),
    SD_BUS_SIGNAL("StatusNotifierItemUnregistered", "s", 0),
    SD_BUS_SIGNAL("StatusNotifierHostRegistered", "", 0),
    SD_BUS_PROPERTY("RegisteredStatusNotifierItems", "as", prop_get_items, 0, SD_BUS_VTABLE_PROPERTY_EMITS_CHANGE),
    SD_BUS_PROPERTY("IsStatusNotifierHostRegistered", "b", prop_get_host_reg, 0, 0),
    SD_BUS_PROPERTY("ProtocolVersion", "i", prop_get_proto_ver, 0, 0),
    SD_BUS_VTABLE_END
};

static int on_name_owner_changed(sd_bus_message *m, void *userdata, sd_bus_error *ret_error) {
    (void)userdata; (void)ret_error;
    const char *name = NULL, *old_owner = NULL, *new_owner = NULL;
    int r = sd_bus_message_read(m, "sss", &name, &old_owner, &new_owner);
    if (r < 0) return 0;

    if (new_owner && new_owner[0] == '\0') {
        for (ssize_t i = (ssize_t)item_count - 1; i >= 0; i--) {
            size_t nlen = name ? strlen(name) : 0;
            size_t olen = old_owner ? strlen(old_owner) : 0;
            if (nlen > 0 && strncmp(items[i], name, nlen) == 0 && items[i][nlen] == '/') {
                remove_item((size_t)i);
            } else if (olen > 0 && strncmp(items[i], old_owner, olen) == 0 && items[i][olen] == '/') {
                remove_item((size_t)i);
            }
        }
    }
    return 0;
}

static int handle_action(const char *action, const char *target) {
    if (!action || !target || !action[0] || !target[0]) return 0;

    init_paths();

    FILE *f = fopen(state_file, "r");
    if (!f) return 0;

    fseek(f, 0, SEEK_END);
    long sz = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (sz <= 0 || sz > 256 * 1024) {
        fclose(f);
        return 0;
    }

    char *buf = malloc((size_t)sz + 1);
    if (!buf) {
        fclose(f);
        return 0;
    }
    size_t rd = fread(buf, 1, (size_t)sz, f);
    fclose(f);
    buf[rd] = '\0';

    char needle[128];
    snprintf(needle, sizeof(needle), "\"%s\": {", target);
    char *entry = strstr(buf, needle);
    if (!entry) {
        free(buf);
        return 0;
    }

    char *line_end = strchr(entry, '\n');
    if (line_end) *line_end = '\0';

    char service[256] = {0};
    char path[512] = {0};

    const char *svc_prefix = "\"service\": \"";
    char *svc_p = strstr(entry, svc_prefix);
    if (svc_p) {
        svc_p += strlen(svc_prefix);
        char *end_q = strchr(svc_p, '"');
        if (end_q && (size_t)(end_q - svc_p) < sizeof(service)) {
            memcpy(service, svc_p, (size_t)(end_q - svc_p));
            service[end_q - svc_p] = '\0';
        }
    }

    const char *path_prefix = "\"path\": \"";
    char *path_p = strstr(entry, path_prefix);
    if (path_p) {
        path_p += strlen(path_prefix);
        char *end_q = strchr(path_p, '"');
        if (end_q && (size_t)(end_q - path_p) < sizeof(path)) {
            memcpy(path, path_p, (size_t)(end_q - path_p));
            path[end_q - path_p] = '\0';
        }
    }

    free(buf);

    if (!service[0] || !path[0]) return 0;

    sd_bus *action_bus = NULL;
    if (sd_bus_default_user(&action_bus) < 0) return 0;

    if (strcmp(action, "activate") == 0) {
        sd_bus_call_method(action_bus, service, path, "org.kde.StatusNotifierItem",
                           "Activate", NULL, NULL, "ii", 0, 0);
    } else if (strcmp(action, "context") == 0) {
        int r = sd_bus_call_method(action_bus, service, path, "org.kde.StatusNotifierItem",
                                   "ContextMenu", NULL, NULL, "ii", 0, 0);
        if (r < 0) {
            sd_bus_call_method(action_bus, service, path, "org.kde.StatusNotifierItem",
                               "SecondaryActivate", NULL, NULL, "ii", 0, 0);
        }
    } else if (strcmp(action, "secondary") == 0) {
        sd_bus_call_method(action_bus, service, path, "org.kde.StatusNotifierItem",
                           "SecondaryActivate", NULL, NULL, "ii", 0, 0);
    } else if (strcmp(action, "scroll_up") == 0) {
        sd_bus_call_method(action_bus, service, path, "org.kde.StatusNotifierItem",
                           "Scroll", NULL, NULL, "is", 1, "vertical");
    } else if (strcmp(action, "scroll_down") == 0) {
        sd_bus_call_method(action_bus, service, path, "org.kde.StatusNotifierItem",
                           "Scroll", NULL, NULL, "is", -1, "vertical");
    }

    sd_bus_flush_close_unref(action_bus);
    return 0;
}

int main(int argc, char **argv) {
    if (argc >= 4 && strcmp(argv[1], "--action") == 0) {
        return handle_action(argv[2], argv[3]);
    }

    signal(SIGINT, handle_sig);
    signal(SIGTERM, handle_sig);

    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--icon-mode") == 0 && i + 1 < argc) {
            opt_icon_mode = argv[++i];
        } else if (strcmp(argv[i], "--icon-width") == 0 && i + 1 < argc) {
            opt_icon_width = atoi(argv[++i]);
        } else if (strcmp(argv[i], "--icon-fit") == 0 && i + 1 < argc) {
            opt_icon_fit = argv[++i];
        } else if (strcmp(argv[i], "--spacing") == 0 && i + 1 < argc) {
            opt_spacing = argv[++i];
        } else if (strcmp(argv[i], "--hide-passive") == 0 && i + 1 < argc) {
            const char *val = argv[++i];
            opt_hide_passive = (strcasecmp(val, "true") == 0 || strcmp(val, "1") == 0);
        }
    }

    init_paths();

    int r = sd_bus_default_user(&bus);
    if (r < 0) {
        fprintf(stderr, "cellbar-tray: cannot connect to user session bus: %s\n", strerror(-r));
        return 1;
    }

    r = sd_bus_add_object_vtable(bus, NULL, "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher",
                                 watcher_vtable, NULL);
    if (r < 0) {
        fprintf(stderr, "cellbar-tray: failed to add StatusNotifierWatcher vtable: %s\n", strerror(-r));
        sd_bus_unref(bus);
        return 1;
    }

    sd_bus_match_signal(bus, NULL, "org.freedesktop.DBus", "/org/freedesktop/DBus",
                        "org.freedesktop.DBus", "NameOwnerChanged", on_name_owner_changed, NULL);

    r = sd_bus_request_name(bus, "org.kde.StatusNotifierWatcher", 0);
    if (r < 0) {
        fprintf(stderr, "cellbar-tray: org.kde.StatusNotifierWatcher already owned on bus.\n");
    }

    // Broadcast host registered so all clients re-announce themselves
    sd_bus_emit_signal(bus, "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher",
                       "StatusNotifierHostRegistered", "");

    // Discover pre-existing items on the session bus
    sd_bus_message *reply = NULL;
    if (sd_bus_call_method(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus",
                           "org.freedesktop.DBus", "ListNames", NULL, &reply, "") >= 0) {
        char **names = NULL;
        if (sd_bus_message_read_strv(reply, &names) >= 0 && names) {
            for (char **p = names; *p; p++) {
                if (strstr(*p, "StatusNotifierItem")) {
                    char full[512];
                    snprintf(full, sizeof(full), "%s/StatusNotifierItem", *p);
                    add_item(full);
                }
            }
        }
        sd_bus_message_unref(reply);
    }

    // Render initial state
    update_and_render();

    // Event loop: completely sleeps when idle with 0% CPU!
    while (running) {
        r = sd_bus_process(bus, NULL);
        if (r < 0) break;
        if (r > 0) continue;
        r = sd_bus_wait(bus, (uint64_t)-1);
        if (r < 0) break;
    }

    for (size_t i = 0; i < item_count; i++) free(items[i]);
    sd_bus_unref(bus);
    return 0;
}
