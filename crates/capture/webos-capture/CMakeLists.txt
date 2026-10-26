cmake_minimum_required(VERSION 3.18)
project(prysm-webos-capture LANGUAGES C CXX)

if(NOT CMAKE_CROSSCOMPILING)
    message(FATAL_ERROR "Use the buildroot-nc4 webOS toolchain (see README.md)")
endif()

include(FetchContent)
FetchContent_Declare(hyperion_capture
    GIT_REPOSITORY https://github.com/webosbrew/hyperion-webos.git
    GIT_TAG 00c932f092084b234b3f619b060a98fa134c12bd
    GIT_SUBMODULES tv-native-apis
    SOURCE_SUBDIR prysm-capture-only
)
# Only use the capture libraries; exclude the Hyperion client and Luna service.
FetchContent_MakeAvailable(hyperion_capture)

set(COMMON_FLAGS "-mcpu=cortex-a9 -mfloat-abi=softfp -mfpu=neon")
add_compile_options(-mcpu=cortex-a9 -mfloat-abi=softfp -mfpu=neon -Wall -Wextra)
add_compile_definitions(_POSIX_C_SOURCE=200809L _DEFAULT_SOURCE)
set(CMAKE_BUILD_RPATH "$ORIGIN")
# The pinned native API projects predate CMake 4's minimum policy version.
set(CMAKE_POLICY_VERSION_MINIMUM 3.5)
add_subdirectory(${hyperion_capture_SOURCE_DIR}/libyuv ${CMAKE_BINARY_DIR}/libyuv)
foreach(api dile-libs libgm libhalgal libvtcapture)
    add_subdirectory(${hyperion_capture_SOURCE_DIR}/tv-native-apis/${api} ${CMAKE_BINARY_DIR}/tv-native-apis/${api})
endforeach()
add_subdirectory(${hyperion_capture_SOURCE_DIR}/unicapture ${CMAKE_BINARY_DIR}/unicapture)

add_executable(prysm-webos-capture main.c transport.c log.c ${hyperion_capture_SOURCE_DIR}/src/utils.c)
target_include_directories(prysm-webos-capture PRIVATE ${hyperion_capture_SOURCE_DIR}/src)
target_link_libraries(prysm-webos-capture PRIVATE unicapture pthread dl rt)
set_target_properties(prysm-webos-capture PROPERTIES ENABLE_EXPORTS ON)
install(TARGETS prysm-webos-capture gm_backend halgal_backend dile_vt_backend vtcapture_backend
    RUNTIME DESTINATION . LIBRARY DESTINATION .)
# Preserve the license for the reused capture sources and libraries.
install(FILES ${hyperion_capture_SOURCE_DIR}/LICENSE DESTINATION licenses RENAME hyperion-webos.txt)
install(FILES ${hyperion_capture_SOURCE_DIR}/tv-native-apis/LICENSE DESTINATION licenses RENAME tv-native-apis.txt)
ExternalProject_Get_Property(libyuv SOURCE_DIR)
install(FILES ${SOURCE_DIR}/LICENSE DESTINATION licenses RENAME libyuv.txt)
