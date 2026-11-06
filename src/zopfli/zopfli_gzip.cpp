/*
Copyright 2013 Google Inc. All Rights Reserved.
Copyright 2015 Mr_KrzYch00. All Rights Reserved.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.

Author: lode.vandevenne@gmail.com (Lode Vandevenne)
Author: jyrki.alakuijala@gmail.com (Jyrki Alakuijala)
*/

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <sys/stat.h>

#include "zopfli.h"
#include "../zlib/zlib.h"
#include "../libdeflate/libdeflate.h"
#include "blocksplitter.h"
#include <stdint.h>
#include <vector>
#include "deflate.h"
#include "zopfli.h"
#include "zlib_container.h"
#include "../main.h"
#include <time.h>

/* Stores a 32-bit gzip header/trailer field in little-endian order. */
static void PutLE32(unsigned char* p, unsigned long v) {
  p[0] = (unsigned char)(v & 0xFF);
  p[1] = (unsigned char)((v >> 8) & 0xFF);
  p[2] = (unsigned char)((v >> 16) & 0xFF);
  p[3] = (unsigned char)((v >> 24) & 0xFF);
}

#define ZOPFLI_APPEND_DATA(/* T */ value, /* T** */ data, /* size_t* */ size) {\
(*data)[(*size)] = (value);\
(*size)++;\
}

static void ZopfliZipCompress(unsigned mode, unsigned multithreading,
                              const unsigned char* in, size_t insize, time_t time, std::string name,
                              unsigned char** out, size_t* outsize) {
  static const unsigned char filePKh[10]     = { 80, 75,  3,  4, 20,  0,  2,  0,  8,  0};
  static const unsigned char CDIRPKh[12]     = { 80, 75,  1,  2, 20,  0, 20,  0,  2,  0,  8,  0};
  static const unsigned char CDIRPKs[12]     = {  0,  0,  0,  0,  0,  0,  0,  0, 32,  0,  0,  0};
  static const unsigned char EndCDIRPKh[12]  = { 80, 75,  5,  6,  0,  0,  0,  0,  1,  0,  1,  0};

  unsigned long crcvalue = crc32(0, in, insize);
  unsigned long i;
  std::string infilename = name.substr(name.find_last_of('/') + 1);
  size_t max = infilename.size();
  *out = (unsigned char*)realloc(*out, 200);

  struct tm* times = localtime(&time);
  unsigned long dostime = times->tm_year < 80 ? 0x00210000 : times->tm_year > 207 ? 0xFF9FBF7D : (
                            (times->tm_year - 80) << 25 | (times->tm_mon + 1) << 21 | times->tm_mday << 16
                            | times->tm_hour << 11 | times->tm_min << 5 | times->tm_sec >> 1
                          );

  /* File PK STATIC DATA + CM */

  for(i=0;i<sizeof(filePKh);++i) ZOPFLI_APPEND_DATA(filePKh[i],out,outsize);

  /* MS-DOS TIME */
  for(i=0;i<4;++i) ZOPFLI_APPEND_DATA((dostime >> (i*8)) % 256, out, outsize);

  /* CRC */
  for(i=0;i<4;++i) ZOPFLI_APPEND_DATA((crcvalue >> (i*8)) % 256, out, outsize);

  /* OSIZE NOT KNOWN YET - WILL UPDATE AFTER COMPRESSION */
  for(i=0;i<4;++i) ZOPFLI_APPEND_DATA(0, out, outsize);

  /* ISIZE */
  for(i=0;i<4;++i) ZOPFLI_APPEND_DATA((insize >> (i*8)) % 256, out, outsize);

  /* FNLENGTH */
  for(i=0;i<2;++i) ZOPFLI_APPEND_DATA((max >> (i*8)) % 256, out, outsize);

  /* NO EXTRA FLAGS */
  for(i=0;i<2;++i) ZOPFLI_APPEND_DATA(0, out, outsize);

  /* FILENAME */
  for(i=0;i<max;++i) ZOPFLI_APPEND_DATA(infilename[i], out, outsize);
  unsigned long rawdeflsize = *outsize;

  ZopfliBuffer(mode, multithreading, in, insize, out, outsize);
  *out = (unsigned char*)realloc(*out, 200 + *outsize);

  rawdeflsize = *outsize - rawdeflsize;

  /* C-DIR PK HEADER STATIC DATA */
  unsigned long cdirsize = *outsize;
  for(i=0;i<sizeof(CDIRPKh);++i) ZOPFLI_APPEND_DATA(CDIRPKh[i],out,outsize);

  /* MS-DOS TIME, CRC, OSIZE, ISIZE FROM */

  for(i=0;i<4;++i) ZOPFLI_APPEND_DATA((dostime >> (i*8)) % 256, out, outsize);

  /* CRC */
  for(i=0;i<4;++i) ZOPFLI_APPEND_DATA((crcvalue >> (i*8)) % 256,out,outsize);

  /* OSIZE + UPDATE IN PK HEADER */
  for(i=0;i<4;++i) ZOPFLI_APPEND_DATA((rawdeflsize >> (i*8)) % 256,out,outsize);
  for(i=0;i<4;++i) (*out)[18+i]=(rawdeflsize >> (i*8)) % 256;

  /* ISIZE */
  for(i=0;i<25;i+=8) ZOPFLI_APPEND_DATA((insize >> i) % 256,out,outsize);

  /* FILENAME LENGTH */
  for(i=0;i<2;++i) ZOPFLI_APPEND_DATA((max >> (i*8)) % 256,out,outsize);

  /* C-DIR STATIC DATA */
  for(i=0;i<sizeof(CDIRPKs);++i) ZOPFLI_APPEND_DATA(CDIRPKs[i],out,outsize);

  /* FilePK offset in ZIP file */
  for(i=0;i<4;++i) ZOPFLI_APPEND_DATA(0,out,outsize);
  unsigned long cdiroffset= rawdeflsize + 30 + max;

  /* FILENAME */
  for(i=0; i<max;++i) ZOPFLI_APPEND_DATA(infilename[i],out,outsize);
  cdirsize = *outsize - cdirsize;

  /* END C-DIR PK STATIC DATA + TOTAL FILES (ALWAYS 1) */
  for(i=0;i<sizeof(EndCDIRPKh);++i) ZOPFLI_APPEND_DATA(EndCDIRPKh[i],out,outsize);

  /* C-DIR SIZE */
  for(i=0;i<4;++i) ZOPFLI_APPEND_DATA((cdirsize >> (i*8)) % 256,out, outsize);

  /* C-DIR OFFSET */
  for(i=0;i<4;++i) ZOPFLI_APPEND_DATA((cdiroffset >> (i*8)) % 256,out, outsize);

  /* NO COMMENTS IN END C-DIR */
  for(i=0;i<2;++i) ZOPFLI_APPEND_DATA(0, out, outsize);
}

/*
Compresses the data according to the gzip specification.
*/
static void ZopfliGzipCompress(unsigned mode, unsigned multithreading,
                        const unsigned char* in, size_t insize, time_t time,
                        unsigned char** out, size_t* outsize, std::string name = "") {
  unsigned crcvalue = crc32(0, in, insize);

  unsigned char has_name = name != "";
  std::string infile_str = name.substr(name.find_last_of('/') + 1);
  unsigned name_size = infile_str.length() + 1;
  const char* infilename = infile_str.c_str();

  (*out) = (unsigned char*)malloc(20);
  (*out)[*outsize] = 31; (*outsize)++;            /* ID1 */
  (*out)[*outsize] = 139; (*outsize)++;           /* ID2 */
  (*out)[*outsize] = 8; (*outsize)++;             /* CM  */
  (*out)[*outsize] = has_name << 3; (*outsize)++; /* FLG */

  /* MTIME */
  PutLE32(&(*out)[*outsize], (unsigned long)time); (*outsize) += 4;

  (*out)[*outsize] = 2; (*outsize)++;  /* XFL, 2 indicates best compression. */
  (*out)[*outsize] = 3; (*outsize)++;  /* OS follows Unix conventions. */

  if (has_name) {
    (*out) = (unsigned char*)realloc(*out, 20 + name_size);
    memcpy((&(*out)[*outsize]), infilename, name_size);
    (*outsize) += name_size;
  }

  ZopfliBuffer(mode, multithreading, in, insize, out, outsize);
  (*out) = (unsigned char*)realloc(*out, *outsize + 8);

  /* CRC */
  PutLE32(&(*out)[*outsize], (unsigned long)crcvalue); (*outsize) += 4;

  /* ISIZE */
  PutLE32(&(*out)[*outsize], (unsigned long)(insize & 0xFFFFFFFFu)); (*outsize) += 4;
}

static void LoadGzip(const char* filename, unsigned char** out, long long* outsize_p) {
  gzFile r = gzopen(filename, "rb");
  if (!r) {
    *outsize_p = -1;
    return;
  }
#define GZIP_READ_SIZE 65536
  size_t alloc_size = GZIP_READ_SIZE;
  *out = (unsigned char*)malloc(alloc_size + 16);
  size_t out_size = 0;
  do {
    int bytes = gzread(r, (*out) + out_size, GZIP_READ_SIZE);
    if (bytes > 0) {
      out_size += bytes;
      if (alloc_size - out_size < GZIP_READ_SIZE) {
        alloc_size *= 2;
        (*out) = (unsigned char*)realloc(*out, alloc_size + 16);
      }
    } else if (bytes == 0) {
      break;
    } else {  // bytes < 0
      fprintf(stderr, "%s: gzip decompression error\n", filename);
      gzclose_r(r);
      *outsize_p = -1;
      return;
    }
  }
  while (!gzeof(r));
  gzclose_r(r);
  *outsize_p = (long long)out_size;
}

/*
 Loads a file into a memory array.
 */
static void LoadFile(const char* filename,
                     unsigned char** out, long long* outsize) {
  FILE* file = fopen(filename, "rb");
  if (!file) {*outsize = -1; return;}

  fseek(file , 0 , SEEK_END);
  *outsize = ftell(file);
  rewind(file);

  *out = (unsigned char*)malloc(*outsize + 16);
  if (!*out && *outsize){
    exit(1);
  }
  if (*outsize) {
    size_t testsize = fread(*out, 1, *outsize, file);
    if ((long long)testsize != *outsize) {
      /* It could be a directory */
      free(*out);
      *out = 0;
      *outsize = -1;
    }
  }

  fclose(file);
}

/*
 Saves a file from a memory array, overwriting the file if it existed.
 */
static void SaveFile(const char* filename,
                     const unsigned char* in, size_t insize) {
  FILE* file = fopen(filename, "wb");
  if (!file){
    fprintf(stderr, "Can't write to file %s\n", filename);
  }
  else {
    fwrite((char*)in, 1, insize, file);
    fclose(file);
  }
}

/*
 outfilename: filename to write output to, or 0 to write to stdout instead
 */
int ZopfliGzip(const char* infilename, const char* outfilename, unsigned mode, unsigned multithreading, unsigned ZIP, unsigned char isGZ, const char* gzip_name) {
  unsigned char* in;
  long long insize = -1;
  unsigned char* out = 0;
  size_t outsize = 0;

  if (isGZ) {
    LoadGzip(infilename, &in, &insize);
  }
  else {
    LoadFile(infilename, &in, &insize);
  }
  if (insize < 0) {
    fprintf(stderr, "Invalid file: %s\n", infilename);
    return EXIT_FAILURE;
  }

  struct stat st;
  stat(infilename, &st);
  time_t time = st.st_mtime;
  if (!ZIP) {
    ZopfliGzipCompress(mode, multithreading, in, insize, time, &out, &outsize, (std::string)(gzip_name ? gzip_name : ""));
  }
  else {
    ZopfliZipCompress(mode, multithreading, in, insize, time, infilename, &out, &outsize);
  }
  free(in);

  SaveFile(outfilename, out, outsize);

  free(out);
  return EXIT_SUCCESS;
}

namespace {

// Reads the LZ77 parse back out of a Deflate stream. Only used on libdeflate's
// own output, but still checks every bound so a bad stream just fails.
struct ParsedDeflate {
  std::vector<unsigned short> litlens, dists;
};

class BitIn {
 public:
  BitIn(const unsigned char* p, size_t n) : p_(p), n_(n) {}
  unsigned Peek(unsigned k) {
    while (cnt_ <= 56) {
      uint64_t b = pos_ < n_ ? p_[pos_] : 0;
      pos_++;
      buf_ |= b << cnt_;
      cnt_ += 8;
    }
    return (unsigned)(buf_ & ((1ull << k) - 1));
  }
  void Drop(unsigned k) { buf_ >>= k; cnt_ -= k; }
  unsigned Get(unsigned k) {
    if (!k) return 0;
    unsigned v = Peek(k);
    Drop(k);
    return v;
  }
  bool Overrun() const { return (pos_ * 8 - cnt_) > n_ * 8; }

 private:
  const unsigned char* p_;
  size_t n_, pos_ = 0;
  uint64_t buf_ = 0;
  unsigned cnt_ = 0;
};

const unsigned kTableBits = 15;

// Fills a lookup table indexed by the next 15 input bits; entries store
// symbol << 4 | code length, 0 for unused codes.
bool BuildTable(const unsigned char* lengths, unsigned n, std::vector<unsigned>& table) {
  unsigned count[16] = {0}, next[16] = {0};
  for (unsigned i = 0; i < n; i++) count[lengths[i]]++;
  count[0] = 0;
  unsigned code = 0;
  for (unsigned b = 1; b < 16; b++) {
    code = (code + count[b - 1]) << 1;
    next[b] = code;
  }
  table.assign(1u << kTableBits, 0);
  for (unsigned s = 0; s < n; s++) {
    unsigned len = lengths[s];
    if (!len) continue;
    unsigned c = next[len]++;
    if (c >= (1u << len)) return false;
    unsigned r = 0;
    for (unsigned j = 0; j < len; j++) r |= ((c >> j) & 1) << (len - 1 - j);
    for (unsigned x = r; x < (1u << kTableBits); x += 1u << len) table[x] = s << 4 | len;
  }
  return true;
}

bool Decode(BitIn& in, const std::vector<unsigned>& table, unsigned* sym) {
  unsigned e = table[in.Peek(kTableBits)];
  if (!e) return false;
  in.Drop(e & 15);
  *sym = e >> 4;
  return true;
}

const unsigned short kLenBase[29] = {3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31,
                                     35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258};
const unsigned char kLenExtra[29] = {0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2,
                                     3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0};
const unsigned short kDistBase[30] = {1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129,
                                      193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097,
                                      6145, 8193, 12289, 16385, 24577};
const unsigned char kDistExtra[30] = {0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6,
                                      6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13};

// Returns false for malformed streams and for streams with stored blocks
// (re-encoding those as Huffman blocks would only make them larger).
bool ParseDeflate(const unsigned char* data, size_t n, size_t insize, ParsedDeflate* p) {
  BitIn in(data, n);
  std::vector<unsigned> lt, dt, ct;
  size_t produced = 0;
  for (;;) {
    unsigned final = in.Get(1);
    unsigned type = in.Get(2);
    unsigned char lengths[320];
    unsigned hlit, hdist;
    if (type == 1) {
      hlit = 288;
      hdist = 32;
      for (unsigned i = 0; i < 288; i++) lengths[i] = i < 144 ? 8 : i < 256 ? 9 : i < 280 ? 7 : 8;
      for (unsigned i = 0; i < 32; i++) lengths[288 + i] = 5;
    } else if (type == 2) {
      hlit = in.Get(5) + 257;
      hdist = in.Get(5) + 1;
      unsigned hclen = in.Get(4) + 4;
      static const unsigned char order[19] = {16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15};
      unsigned char cl[19] = {0};
      for (unsigned i = 0; i < hclen; i++) cl[order[i]] = in.Get(3);
      if (!BuildTable(cl, 19, ct)) return false;
      unsigned i = 0;
      while (i < hlit + hdist) {
        unsigned sym;
        if (!Decode(in, ct, &sym)) return false;
        if (sym < 16) {
          lengths[i++] = sym;
          continue;
        }
        unsigned rep, val = 0;
        if (sym == 16) {
          if (!i) return false;
          val = lengths[i - 1];
          rep = 3 + in.Get(2);
        } else if (sym == 17) {
          rep = 3 + in.Get(3);
        } else {
          rep = 11 + in.Get(7);
        }
        if (i + rep > hlit + hdist) return false;
        while (rep--) lengths[i++] = val;
      }
    } else {
      return false;
    }
    if (!BuildTable(lengths, hlit, lt) || !BuildTable(lengths + hlit, hdist, dt)) return false;
    for (;;) {
      unsigned sym;
      if (!Decode(in, lt, &sym)) return false;
      if (sym < 256) {
        p->litlens.push_back(sym);
        p->dists.push_back(0);
        produced++;
        continue;
      }
      if (sym == 256) break;
      sym -= 257;
      if (sym >= 29) return false;
      unsigned len = kLenBase[sym] + in.Get(kLenExtra[sym]);
      unsigned ds;
      if (!Decode(in, dt, &ds) || ds >= 30) return false;
      unsigned dist = kDistBase[ds] + in.Get(kDistExtra[ds]);
      if (dist > produced) return false;
      p->litlens.push_back(len);
      p->dists.push_back(dist);
      produced += len;
    }
    if (in.Overrun() || produced > insize) return false;
    if (final) break;
  }
  return produced == insize;
}

// Rewrites the Deflate stream at (*out)[start..*outsize): keeps its LZ77
// parse but re-splits it into blocks with Zopfli's block splitter and gives
// every block Zopfli's Huffman code and header optimization. The result is
// kept if it is smaller.
void Reencode(size_t insize, unsigned char** out, size_t* outsize, size_t start) {
  ParsedDeflate p;
  if (!ParseDeflate(*out + start, *outsize - start, insize, &p) || p.litlens.empty()) return;
  ZopfliOptions options;
  ZopfliInitOptions(&options, 2, 0, 0);
  size_t* points = 0;
  size_t npoints = 0;
  ZopfliLZ77Store store;
  store.litlens = p.litlens.data();
  store.dists = p.dists.data();
  store.size = p.litlens.size();
  store.symbols = 0;
  ZopfliSplitLZ77Store(&options, &store, &points, &npoints);
  unsigned char bp = 0;
  unsigned char* re = 0;
  size_t resize = 0;
  ZopfliDeflateLZ77(&options, p.litlens.data(), p.dists.data(), p.litlens.size(), points, npoints, &bp, &re,
                    &resize);
  free(points);
  if (resize < *outsize - start) {
    memcpy(*out + start, re, resize);
    *outsize = start + resize;
  }
  free(re);
}

}  // namespace

// Appends a raw Deflate stream of `in` produced by libdeflate at `level` to
// (*out, *outsize), optionally re-encoded by Reencode(). Level 10 is
// libdeflate's fastest near-optimal parse.
void FastDeflateRaw(const unsigned char* in, size_t insize, unsigned char** out, size_t* outsize, int level,
                    bool reencode) {
  struct libdeflate_compressor* c = libdeflate_alloc_compressor(level);
  if (!c) {
    fprintf(stderr, "Error: out of memory\n");
    exit(EXIT_FAILURE);
  }
  size_t bound = libdeflate_deflate_compress_bound(c, insize);
  unsigned char* grown = (unsigned char*)realloc(*out, *outsize + bound);
  if (!grown) {
    fprintf(stderr, "Error: out of memory\n");
    exit(EXIT_FAILURE);
  }
  *out = grown;
  size_t n = libdeflate_deflate_compress(c, in, insize, *out + *outsize, bound);
  libdeflate_free_compressor(c);
  if (!n) {
    fprintf(stderr, "Error: libdeflate compression failed\n");
    exit(EXIT_FAILURE);
  }
  size_t start = *outsize;
  *outsize += n;
  if (reencode) Reencode(insize, out, outsize, start);
}

// Appends a raw Deflate stream of `in` to (*out, *outsize), which must be
// either (nullptr, 0) or a malloc'ed buffer. Every caller (gzip, ZIP
// creation, ZIP re-optimization) goes through here, so all of them get the
// same handling of mode 1.
void ZopfliBuffer(unsigned mode, unsigned multithreading, const unsigned char* in, size_t insize, unsigned char** out, size_t* outsize) {
  // At -1 and -2, libdeflate's parse re-encoded with Zopfli's block splitting
  // and Huffman coding is both smaller and faster than Zopfli's own parse
  // (14 gzip files: -2 2,494,361 bytes in 1.1 s, where Zopfli took 1.5 s for
  // 2,502,286). -3 and up keep Zopfli: its optimal parse still wins clearly
  // on some files (up to 1.6% on ptt5). Mode 1 is never passed to
  // ZopfliInitOptions(), which leaves Zopfli unconfigured for it.
  if (mode % 10000 < 3) {
    FastDeflateRaw(in, insize, out, outsize, mode % 10000 < 2 ? 10 : 11, true);
    return;
  }

  ZopfliOptions options;
  ZopfliInitOptions(&options, mode, multithreading, 0);
  unsigned char bp = 0;
  ZopfliDeflate(&options, 1, in, insize, &bp, out, outsize);
}

