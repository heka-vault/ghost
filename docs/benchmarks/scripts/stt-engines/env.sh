# Source before building: the Vulkan headers, the loader's link and glslc, unpacked from
# Ubuntu 26.04's own libvulkan-dev, glslc, libshaderc1 and spirv-headers packages into a user prefix
# (apt-get download + dpkg -x; no root). See the report's method.
SDK="$HOME/.cache/ghost-stt-bench/vulkan-sdk/root/usr"
export TRANSCRIBE_CMAKE_ARGS="-DVulkan_INCLUDE_DIR=$SDK/include -DVulkan_LIBRARY=$SDK/lib/x86_64-linux-gnu/libvulkan.so -DVulkan_GLSLC_EXECUTABLE=$SDK/bin/glslc -DCMAKE_PREFIX_PATH=$SDK"
export PATH="$SDK/bin:$PATH"
export LIBRARY_PATH="$SDK/lib/x86_64-linux-gnu${LIBRARY_PATH:+:$LIBRARY_PATH}"
export CARGO_TARGET_DIR="$HOME/.cache/ghost-stt-bench/target"
