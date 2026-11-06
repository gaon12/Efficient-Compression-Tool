cmake_minimum_required(VERSION 3.10 FATAL_ERROR)
project(libdeflate LANGUAGES C)

# Only the compressor of libdeflate 1.26 (https://github.com/ebiggers/libdeflate,
# MIT license, see COPYING), used for ECT's -1 and for PNGs at -2.
add_library(libdeflate
	lib/deflate_compress.c
	lib/utils.c
	lib/x86/cpu_features.c
	lib/arm/cpu_features.c
	libdeflate.h)

add_library(libdeflate::libdeflate ALIAS libdeflate)

target_include_directories(libdeflate INTERFACE ${CMAKE_CURRENT_SOURCE_DIR})
