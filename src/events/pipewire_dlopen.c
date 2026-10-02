#define _GNU_SOURCE
#include <stddef.h>
#include <stdint.h>
#include <dlfcn.h>
#include <pthread.h>

struct pw_loop;
struct pw_context;
struct pw_core;
struct pw_main_loop;
struct pw_proxy;
struct pw_properties;
struct spa_dict;

static void *pw_lib_handle = NULL;
static pthread_once_t pw_init_once = PTHREAD_ONCE_INIT;

static void (*sym_pw_init)(int *argc, char **argv[]) = NULL;
static struct pw_context * (*sym_pw_context_new)(struct pw_loop *main_loop, struct spa_dict *props, size_t user_data_size) = NULL;
static struct pw_core * (*sym_pw_context_connect)(struct pw_context *context, struct spa_dict *props, size_t user_data_size) = NULL;
static void (*sym_pw_context_destroy)(struct pw_context *context) = NULL;
static int (*sym_pw_core_disconnect)(struct pw_core *core) = NULL;
static struct pw_main_loop * (*sym_pw_main_loop_new)(const struct spa_dict *props) = NULL;
static struct pw_loop * (*sym_pw_main_loop_get_loop)(struct pw_main_loop *loop) = NULL;
static int (*sym_pw_main_loop_run)(struct pw_main_loop *loop) = NULL;
static int (*sym_pw_main_loop_quit)(struct pw_main_loop *loop) = NULL;
static void (*sym_pw_main_loop_destroy)(struct pw_main_loop *loop) = NULL;
static const char * (*sym_pw_proxy_get_type)(struct pw_proxy *proxy, uint32_t *version) = NULL;
static void (*sym_pw_proxy_destroy)(struct pw_proxy *proxy) = NULL;
static void (*sym_pw_properties_free)(struct pw_properties *properties) = NULL;
static struct pw_properties * (*sym_pw_properties_new_string)(const char *args) = NULL;

static void do_pw_dlopen(void) {
    pw_lib_handle = dlopen("libpipewire-0.3.so.0", RTLD_NOW | RTLD_GLOBAL);
    if (!pw_lib_handle) {
        pw_lib_handle = dlopen("libpipewire-0.3.so", RTLD_NOW | RTLD_GLOBAL);
    }
    if (!pw_lib_handle) {
        return;
    }

    sym_pw_init = (void (*)(int *, char **[]))dlsym(pw_lib_handle, "pw_init");
    sym_pw_context_new = (struct pw_context * (*)(struct pw_loop *, struct spa_dict *, size_t))dlsym(pw_lib_handle, "pw_context_new");
    sym_pw_context_connect = (struct pw_core * (*)(struct pw_context *, struct spa_dict *, size_t))dlsym(pw_lib_handle, "pw_context_connect");
    sym_pw_context_destroy = (void (*)(struct pw_context *))dlsym(pw_lib_handle, "pw_context_destroy");
    sym_pw_core_disconnect = (int (*)(struct pw_core *))dlsym(pw_lib_handle, "pw_core_disconnect");
    sym_pw_main_loop_new = (struct pw_main_loop * (*)(const struct spa_dict *))dlsym(pw_lib_handle, "pw_main_loop_new");
    sym_pw_main_loop_get_loop = (struct pw_loop * (*)(struct pw_main_loop *))dlsym(pw_lib_handle, "pw_main_loop_get_loop");
    sym_pw_main_loop_run = (int (*)(struct pw_main_loop *))dlsym(pw_lib_handle, "pw_main_loop_run");
    sym_pw_main_loop_quit = (int (*)(struct pw_main_loop *))dlsym(pw_lib_handle, "pw_main_loop_quit");
    sym_pw_main_loop_destroy = (void (*)(struct pw_main_loop *))dlsym(pw_lib_handle, "pw_main_loop_destroy");
    sym_pw_proxy_get_type = (const char * (*)(struct pw_proxy *, uint32_t *))dlsym(pw_lib_handle, "pw_proxy_get_type");
    sym_pw_proxy_destroy = (void (*)(struct pw_proxy *))dlsym(pw_lib_handle, "pw_proxy_destroy");
    sym_pw_properties_free = (void (*)(struct pw_properties *))dlsym(pw_lib_handle, "pw_properties_free");
    sym_pw_properties_new_string = (struct pw_properties * (*)(const char *))dlsym(pw_lib_handle, "pw_properties_new_string");
}

static inline void ensure_loaded(void) {
    pthread_once(&pw_init_once, do_pw_dlopen);
}

void pw_init(int *argc, char **argv[]) {
    ensure_loaded();
    if (sym_pw_init) {
        sym_pw_init(argc, argv);
    }
}

struct pw_context * pw_context_new(struct pw_loop *main_loop, struct spa_dict *props, size_t user_data_size) {
    ensure_loaded();
    if (!props && sym_pw_properties_new_string) {
        props = (struct spa_dict *)sym_pw_properties_new_string(
            "module.rt = false\n"
            "module.client-node = false\n"
            "module.client-device = false\n"
            "module.adapter = false\n"
            "module.session-manager = false\n"
        );
    }
    return sym_pw_context_new ? sym_pw_context_new(main_loop, props, user_data_size) : NULL;
}

struct pw_core * pw_context_connect(struct pw_context *context, struct spa_dict *props, size_t user_data_size) {
    ensure_loaded();
    return sym_pw_context_connect ? sym_pw_context_connect(context, props, user_data_size) : NULL;
}

void pw_context_destroy(struct pw_context *context) {
    ensure_loaded();
    if (sym_pw_context_destroy) {
        sym_pw_context_destroy(context);
    }
}

int pw_core_disconnect(struct pw_core *core) {
    ensure_loaded();
    return sym_pw_core_disconnect ? sym_pw_core_disconnect(core) : -1;
}

struct pw_main_loop * pw_main_loop_new(const struct spa_dict *props) {
    ensure_loaded();
    return sym_pw_main_loop_new ? sym_pw_main_loop_new(props) : NULL;
}

struct pw_loop * pw_main_loop_get_loop(struct pw_main_loop *loop) {
    ensure_loaded();
    return sym_pw_main_loop_get_loop ? sym_pw_main_loop_get_loop(loop) : NULL;
}

int pw_main_loop_run(struct pw_main_loop *loop) {
    ensure_loaded();
    return sym_pw_main_loop_run ? sym_pw_main_loop_run(loop) : -1;
}

int pw_main_loop_quit(struct pw_main_loop *loop) {
    ensure_loaded();
    return sym_pw_main_loop_quit ? sym_pw_main_loop_quit(loop) : -1;
}

void pw_main_loop_destroy(struct pw_main_loop *loop) {
    ensure_loaded();
    if (sym_pw_main_loop_destroy) {
        sym_pw_main_loop_destroy(loop);
    }
}

const char * pw_proxy_get_type(struct pw_proxy *proxy, uint32_t *version) {
    ensure_loaded();
    return sym_pw_proxy_get_type ? sym_pw_proxy_get_type(proxy, version) : NULL;
}

void pw_proxy_destroy(struct pw_proxy *proxy) {
    ensure_loaded();
    if (sym_pw_proxy_destroy) {
        sym_pw_proxy_destroy(proxy);
    }
}

void pw_properties_free(struct pw_properties *properties) {
    ensure_loaded();
    if (sym_pw_properties_free) {
        sym_pw_properties_free(properties);
    }
}
