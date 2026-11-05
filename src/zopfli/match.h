//
//  match.h
//  Efficient Compression Tool
//
//  Created by Felix Hanau on 31.12.15.
//  Copyright © 2015 Felix Hanau.
//

/*
 Finds how long the match of scan and match is. Can be used to find how many
 bytes starting from scan, and from match, are equal. Returns the last byte
 after scan, which is still equal to the correspondinb byte after match.
 scan is the position to compare
 match is the earlier position to compare.
 end is the last possible byte, beyond which to stop looking.
 safe_end is unused and kept for compatibility with existing callers.

 The comparison runs eight bytes at a time through memcpy loads, so it is
 safe for any alignment, and it never reads at or beyond end. The previous
 version (based on cloudflare's zlib fork) dereferenced casted pointers and
 read up to 15 bytes past end. The loop structure follows clevert-app/ect
 ("Modify GetMatch, avoid out of bound access, fix more cast-align UB"),
 where it benchmarked as fast as the original on x86-64 and arm64.
 */
#ifndef ZOPFLI_MATCH_H_
#define ZOPFLI_MATCH_H_

#include <stdint.h>
#include "util.h"

#if defined(_MSC_VER) && !defined(__clang__) && (defined(_M_X64) || defined(_M_ARM64))
#include <intrin.h>
#endif

#if defined(__BYTE_ORDER__) && __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
#define ZOPFLI_MATCH_WORDS 0
#elif defined(__GNUC__) || defined(__clang__)
#define ZOPFLI_MATCH_WORDS 1
/* Index of the first differing byte of two little-endian words (x != 0). */
static inline unsigned ZopfliFirstDiff64(uint64_t x) { return (unsigned)__builtin_ctzll(x) >> 3; }
#elif defined(_MSC_VER) && (defined(_M_X64) || defined(_M_ARM64))
#define ZOPFLI_MATCH_WORDS 1
static __forceinline unsigned ZopfliFirstDiff64(uint64_t x) {
  unsigned long i;
  _BitScanForward64(&i, x);
  return (unsigned)i >> 3;
}
#else
#define ZOPFLI_MATCH_WORDS 0
#endif

#ifdef __GNUC__
__attribute__ ((always_inline, hot))
#elif defined(_MSC_VER)
__forceinline
#endif
static inline const unsigned char* GetMatch(const unsigned char* scan,
                                            const unsigned char* match,
                                            const unsigned char* end,
                                            const unsigned char* safe_end) {
  (void)safe_end;
#if ZOPFLI_MATCH_WORDS
  while (end - scan >= 8) {
    uint64_t x = ZopfliRead64(scan) ^ ZopfliRead64(match);
    if (x) {
      return scan + ZopfliFirstDiff64(x);
    }
    scan += 8;
    match += 8;
  }
  if (end - scan >= 4) {
    uint32_t x = ZopfliRead32(scan) ^ ZopfliRead32(match);
    if (x) {
      return scan + ZopfliFirstDiff64(x);
    }
    scan += 4;
    match += 4;
  }
#endif
  while (scan < end && *scan == *match) {
    scan++;
    match++;
  }
  return scan;
}

#endif
