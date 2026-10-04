#include <node_api.h>
#include <stdint.h>
static napi_value address_of(napi_env env, napi_callback_info info) {
  size_t argc = 1; napi_value argv[1];
  napi_get_cb_info(env, info, &argc, argv, NULL, NULL);
  void *data = NULL; size_t len = 0;
  napi_get_arraybuffer_info(env, argv[0], &data, &len);
  napi_value r; napi_create_bigint_uint64(env, (uint64_t)(uintptr_t)data, &r); return r;
}
static napi_value deref(napi_env env, napi_callback_info info) {
  size_t argc = 1; napi_value argv[1];
  napi_get_cb_info(env, info, &argc, argv, NULL, NULL);
  void *data = NULL; size_t len = 0;
  napi_get_arraybuffer_info(env, argv[0], &data, &len);
  volatile uint64_t *p = *(volatile uint64_t * volatile *)data;
  uint64_t v = *p;
  napi_value r; napi_create_bigint_uint64(env, v, &r); return r;
}
NAPI_MODULE_INIT() {
  napi_property_descriptor d[] = {
    { "addressOf", NULL, address_of, NULL, NULL, NULL, napi_default, NULL },
    { "deref", NULL, deref, NULL, NULL, NULL, napi_default, NULL },
  };
  napi_define_properties(env, exports, 2, d);
  return exports;
}
