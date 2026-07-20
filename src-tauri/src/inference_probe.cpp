struct ggml_backend_device;
struct ggml_backend;

using ggml_backend_dev_t = ggml_backend_device *;
using ggml_backend_t = ggml_backend *;

extern "C" ggml_backend_t ggml_backend_dev_init(ggml_backend_dev_t device, const char * params);
extern "C" void ggml_backend_free(ggml_backend_t backend);

// Keep C++ exceptions from crossing the Rust FFI boundary. Some Vulkan driver
// failures can throw inside the bundled GGML backend instead of returning null.
extern "C" int fing_probe_ggml_backend(ggml_backend_dev_t device) noexcept {
    try {
        ggml_backend_t backend = ggml_backend_dev_init(device, nullptr);
        if (backend == nullptr) {
            return 0;
        }
        ggml_backend_free(backend);
        return 1;
    } catch (...) {
        return -1;
    }
}
