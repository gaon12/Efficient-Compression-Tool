cmake_minimum_required(VERSION 3.10 FATAL_ERROR)
project(ect VERSION 0.12.1 LANGUAGES CXX C)
cmake_policy(SET CMP0048 NEW)
# Allow use of ZLIB_ROOT
cmake_policy(SET CMP0074 NEW)

include(GNUInstallDirs)

# C++17 is required for <filesystem>/directory support.
set (CMAKE_CXX_STANDARD 17)

if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release)
endif()

# Check that submodules are present only if source was downloaded with git
if(EXISTS "${CMAKE_SOURCE_DIR}/../.git" AND NOT EXISTS "${CMAKE_SOURCE_DIR}/../src/libpng/README")
    message (FATAL_ERROR "Submodules are not initialized. Run \n\tgit submodule update --init --recursive\n within the repository")
endif()

add_executable(ect
  main.cpp
  gztools.cpp
  jpegtran.cpp
  LzFind.c
  support.cpp
  zopflipng.cpp
  # Add headers so they get added to things like Xcode projects
  gztools.h
  LzFind.h
  main.h
  pngusr.h
  support.h
  mozjpeg/transupp.c)

add_executable(ect::ect ALIAS ect)

if(MINGW)
  add_compile_options($<$<COMPILE_LANGUAGE:CXX>:-mno-ms-bitfields>)
  add_compile_options($<$<COMPILE_LANGUAGE:C>:-mno-ms-bitfields>)
endif()

if(WIN32)
  target_sources(ect PRIVATE ect.manifest.rc)
  if(MSVC)
    target_link_options(ect PRIVATE "/MANIFEST:NO")
  endif()
endif()

if(NOT MSVC)
  set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -Wall -Wno-sign-compare -Wno-unused -Wno-unused-parameter")
  set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -Wall -Wno-sign-compare -Wno-unused -Wno-unused-parameter")
endif()

# Use -O3 in release builds
foreach(var CMAKE_C_FLAGS_RELEASE CMAKE_C_FLAGS_RELWITHDEBINFO CMAKE_CXX_FLAGS_RELEASE CMAKE_CXX_FLAGS_RELWITHDEBINFO)
  if(${var} MATCHES "-O2")
    string(REGEX REPLACE "-O2" "-O3" ${var} "${${var}}")
  endif()
endforeach()

# Detect CPU type and whether we're building 64-bit or 32-bit code, courtesy of mozjpeg
math(EXPR BITS "${CMAKE_SIZEOF_VOID_P} * 8")
string(TOLOWER ${CMAKE_SYSTEM_PROCESSOR} CMAKE_SYSTEM_PROCESSOR_LC)
set(COUNT 1)
foreach(ARCH ${CMAKE_OSX_ARCHITECTURES})
  if(COUNT GREATER 1)
    message(FATAL_ERROR "The libjpeg-turbo build system does not support multiple values in CMAKE_OSX_ARCHITECTURES.")
  endif()
  math(EXPR COUNT "${COUNT}+1")
endforeach()
if(CMAKE_SYSTEM_PROCESSOR_LC MATCHES "x86_64" OR
    CMAKE_SYSTEM_PROCESSOR_LC MATCHES "amd64" OR
    CMAKE_SYSTEM_PROCESSOR_LC MATCHES "i[0-9]86" OR
    CMAKE_SYSTEM_PROCESSOR_LC MATCHES "x86" OR
    CMAKE_SYSTEM_PROCESSOR_LC MATCHES "ia32")
  if(BITS EQUAL 64 OR CMAKE_C_COMPILER_ABI MATCHES "ELF X32")
    set(CPU_TYPE x86_64)
  else()
    set(CPU_TYPE i386)
  endif()
  if(NOT CMAKE_SYSTEM_PROCESSOR STREQUAL ${CPU_TYPE})
    set(CMAKE_SYSTEM_PROCESSOR ${CPU_TYPE})
  endif()
elseif(CMAKE_SYSTEM_PROCESSOR_LC STREQUAL "aarch64" OR
    CMAKE_SYSTEM_PROCESSOR_LC MATCHES "^arm")
  if(BITS EQUAL 64)
    set(CPU_TYPE arm64)
  else()
    set(CPU_TYPE arm)
  endif()
else()
  set(CPU_TYPE ${CMAKE_SYSTEM_PROCESSOR_LC})
endif()
if(CMAKE_OSX_ARCHITECTURES MATCHES "x86_64" OR
    CMAKE_OSX_ARCHITECTURES MATCHES "arm64" OR
    CMAKE_OSX_ARCHITECTURES MATCHES "i386")
  set(CPU_TYPE ${CMAKE_OSX_ARCHITECTURES})
endif()
if(MSVC_IDE AND CMAKE_GENERATOR_PLATFORM MATCHES "arm64")
  set(CPU_TYPE arm64)
endif()

#Enable vector extensions. May also add these for ARM if needed.
#32-bit x86 is no longer officially supported and might not work, but do enable the compile flags for it.
if(CPU_TYPE STREQUAL "x86_64" OR CPU_TYPE STREQUAL "i386")
  add_definitions(-DADLER32_SIMD_SSSE3)
  add_definitions(-DINFLATE_CHUNK_SIMD_SSE2)
  add_definitions(-DINFLATE_CHUNK_READ_64LE)
  add_definitions(-DHAS_PCLMUL)
elseif(CPU_TYPE STREQUAL arm64)
  add_definitions(-DADLER32_SIMD_NEON)
  add_definitions(-DINFLATE_CHUNK_SIMD_NEON)
  add_definitions(-DINFLATE_CHUNK_READ_64LE)
  add_definitions(-DPNG_ALIGNED_MEMORY_SUPPORTED)
  set(PNG_ARM_NEON "on")
endif()

if (CMAKE_CXX_COMPILER_ID STREQUAL "GNU" OR CMAKE_CXX_COMPILER_ID STREQUAL "Clang"
    OR CMAKE_CXX_COMPILER_ID STREQUAL "AppleClang" OR CMAKE_CXX_COMPILER_ID STREQUAL "ARMClang")
  if(CPU_TYPE STREQUAL "x86_64" OR CPU_TYPE STREQUAL "i386")
    set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -mpclmul -msse4.2")
    set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -mpclmul -msse4.2")
  elseif(CPU_TYPE STREQUAL arm64)
    set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -march=armv8-a+crc")
    set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -march=armv8-a+crc")
  endif()
endif()

# Optional: target the x86-64-v3 micro-architecture level (AVX2, BMI1/2,
# FMA, LZCNT, MOVBE; Intel Haswell / AMD Excavator, 2013-2015, and newer).
# The binary then refuses to run on older CPUs, so this is off by default
# and the release binaries do not use it. The output is byte-identical; on
# an Intel Core Ultra 5 325 (Linux, GCC) `-9` became ~30% faster for PNGs
# and ~10% faster for gzip, while `-3` and below were unchanged.
option(ECT_X86_64_V3 "Optimize for x86-64-v3 (AVX2) CPUs; not portable to older CPUs" OFF)
if(ECT_X86_64_V3)
  if(NOT CPU_TYPE STREQUAL "x86_64")
    message(FATAL_ERROR "ECT_X86_64_V3 requires an x86-64 target (CPU_TYPE is ${CPU_TYPE})")
  endif()
  # Set through the C/C++ flag variables (like -msse4.2 above) so that it
  # reaches every target, including `ect` defined earlier in this file, but
  # not NASM (mozjpeg SIMD sources).
  if(MSVC)
    # MSVC has no level switch; /arch:AVX2 enables the same instruction set.
    set(ECT_V3_FLAG "/arch:AVX2")
  else()
    set(ECT_V3_FLAG "-march=x86-64-v3")
  endif()
  set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} ${ECT_V3_FLAG}")
  set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} ${ECT_V3_FLAG}")
endif()

# Enable ZLIB_CONST for everything
add_definitions(-DZLIB_CONST)

option(ECT_MULTITHREADING "Enable multithreaded processing support" ON)
option(ECT_MP3_SUPPORT "Enable MP3 support (not currently working)" OFF)
option(ECT_WEBP_SUPPORT "Enable WebP optimization support" ON)

add_subdirectory(leanify EXCLUDE_FROM_ALL)
add_subdirectory(libdeflate EXCLUDE_FROM_ALL)
add_subdirectory(lodepng EXCLUDE_FROM_ALL)
add_subdirectory(miniz EXCLUDE_FROM_ALL)
add_subdirectory(zlib EXCLUDE_FROM_ALL)

# libpng calls find_package(ZLIB REQUIRED). Without intervention CMake's
# FindZLIB locates the *system* zlib: on Linux/macOS libpng was silently
# linked against it instead of the bundled, optimized copy, and on Windows,
# where there is usually no system zlib, configuration failed outright.
# Expose the bundled `zlib` target as ZLIB::ZLIB (FindZLIB keeps an existing
# target) and pre-seed the cache variables FindZLIB checks, so every
# find_package(ZLIB) call resolves to the bundled library.
target_include_directories(zlib INTERFACE ${CMAKE_CURRENT_SOURCE_DIR}/zlib)
add_library(ZLIB::ZLIB ALIAS zlib)
set(ZLIB_INCLUDE_DIR ${CMAKE_CURRENT_SOURCE_DIR}/zlib CACHE PATH "Bundled zlib headers" FORCE)
set(ZLIB_LIBRARY zlib CACHE STRING "Bundled zlib target" FORCE)

# The bundled libpng is only linked statically into ect; skip its install and
# export rules (they cannot export a dependency on the bundled zlib target).
set(SKIP_INSTALL_ALL ON)
add_subdirectory(zopfli EXCLUDE_FROM_ALL)
add_subdirectory(optipng EXCLUDE_FROM_ALL)
# libpng is built with PNG_USER_CONFIG, so pngpriv.h includes "pngusr.h".
# Provide it through an include directory in the build tree instead of
# copying it into the libpng submodule, which left the submodule dirty after
# every configure.
file(COPY ${CMAKE_CURRENT_SOURCE_DIR}/pngusr.h DESTINATION ${CMAKE_CURRENT_BINARY_DIR}/pngusr)
target_include_directories(png_static PRIVATE ${CMAKE_CURRENT_BINARY_DIR}/pngusr)
# Mozjpeg changes the install prefix if it thinks the current is defaulted
set(CMAKE_INSTALL_PREFIX_INITIALIZED_TO_DEFAULT FALSE)
add_subdirectory(mozjpeg EXCLUDE_FROM_ALL)

target_link_libraries(ect
  leanify::leanify
  libdeflate::libdeflate
  lodepng::lodepng
  miniz::miniz
  optipng::optipng
  zopfli::zopfli
  zlib::zlib
  jpeg-static
)

if(ECT_WEBP_SUPPORT)
  find_path(WEBP_INCLUDE_DIR webp/decode.h)
  find_library(WEBP_LIBRARY NAMES webp libwebp)
  find_library(WEBPMUX_LIBRARY NAMES webpmux libwebpmux)
  if(WEBP_INCLUDE_DIR AND WEBP_LIBRARY AND WEBPMUX_LIBRARY)
    # SYSTEM AFTER: the WebP prefix (e.g. /opt/homebrew/include) often also
    # holds the jpeglib.h/jconfig.h of a system libjpeg-turbo. Searching it
    # last keeps the bundled mozjpeg headers in effect; otherwise transupp.c
    # is compiled against another JPEG_LIB_VERSION and fails to link
    # (undefined jpeg_core_output_dimensions).
    target_include_directories(ect SYSTEM AFTER PRIVATE ${WEBP_INCLUDE_DIR})
    target_link_libraries(ect ${WEBP_LIBRARY} ${WEBPMUX_LIBRARY})
    target_compile_definitions(ect PRIVATE WEBP_SUPPORTED=1)
  else()
    message(WARNING "WebP development libraries not found. Building without WebP optimization support.")
  endif()
endif()

# mozjpeg generates some header files that we need to be able to include
target_include_directories(ect
  PRIVATE
  ${CMAKE_CURRENT_BINARY_DIR}/mozjpeg
)

if(NOT ECT_MULTITHREADING)
  target_compile_definitions(ect
    PRIVATE
    NOMULTI=1)
else()
  find_package(Threads REQUIRED)
  target_link_libraries(ect
    Threads::Threads)
endif()

if(ECT_MP3_SUPPORT)
  target_compile_definitions(ect
    PRIVATE
    MP3_SUPPORTED=1)
endif()

install(TARGETS ect RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
