// Copyright 2013 Google Inc. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// Author: lode.vandevenne@gmail.com (Lode Vandevenne)
// Author: jyrki.alakuijala@gmail.com (Jyrki Alakuijala)

// Command line tool to recompress and optimize PNG images, using zopflipng_lib.

/*Modified by Felix Hanau*/

#include <cstdio>
#include <cassert>
#include <set>
#include <unordered_set>
#include <vector>
#include <string>

#include "lodepng/lodepng_util.h"
#include "zopfli/deflate.h"
#include "main.h"
#include "lodepng/lodepng.h"
#include "palette_order.h"
#include <unordered_map>
#include "zlib/zlib.h"
#include <cmath>
#include <climits>
#include <cstdlib>

struct ZopfliPNGOptions {
  ZopfliPNGOptions();

  unsigned Mode;

  // Allow altering hidden colors of fully transparent pixels
  bool lossy_transparent;

  // Convert 16-bit per channel images to 8-bit per channel
  bool lossy_8bit;

  // remove PNG chunks
  bool strip;

  //Use per block multithreading
  unsigned multithreading;

  unsigned quiet;
};

ZopfliPNGOptions::ZopfliPNGOptions()
: lossy_transparent(true)
, lossy_8bit(false)
, strip(false)
{
}

// Deflate compressor passed as function pointer to LodePNG to have it use Zopfli
// as its compression backend.
static unsigned CustomPNGDeflate(unsigned char** out, size_t* outsize, const unsigned char* in, size_t insize, const LodePNGCompressSettings* settings) {
  const ZopfliPNGOptions* png_options = static_cast<const ZopfliPNGOptions*>(settings->custom_context);
  /* libdeflate's near-optimal parse beats one Zopfli iteration on filtered
     image data: at level 11 PNG -2 gets 0.2-0.6% smaller and 7-17% faster
     than with Zopfli -2. -3 and up keep Zopfli, which compresses better
     for the time there. */
  if (png_options->Mode % 10000 < 2) {
    FastDeflateRaw(in, insize, out, outsize);
    return 0;
  }
  if (png_options->Mode % 10000 == 2) {
    FastDeflateRaw(in, insize, out, outsize, 11);
    return 0;
  }
  unsigned char bp = 0;
  ZopfliOptions options;
  ZopfliInitOptions(&options, png_options->Mode, png_options->multithreading, 1);
  ZopfliDeflate(&options, 1, in, insize, &bp, out, outsize);
  return 0;
}

// Returns 32-bit integer value for RGBA color.
static unsigned ColorIndex(const unsigned char* color) {
  return color[0] + (color[1] << 8) + (color[2] << 16) + (color[3] << 24);
}

// Counts amount of colors in the image, up to 257. If transparent_counts_as_one
// is enabled, any color with alpha channel 0 is treated as a single color with
// index 0.
static void CountColors(std::unordered_set<unsigned>* unique, const unsigned char* image, unsigned w, unsigned h) {
  unique->reserve(512);
  unsigned prev = ColorIndex(image) + 1;
  for (size_t i = 0; i < w * h; i++) {
    unsigned index = ColorIndex(&image[i * 4]);
    if (image[i * 4 + 3] == 0) index = 0;
    if(prev != index) {
      unique->insert(index);
      prev = index;
      if (unique->size() > 256) break;
    }
  }
}

static unsigned FindUnusedColor(unsigned char* image, unsigned w, unsigned h) {
  std::set<unsigned> opaque;
  unsigned prev = 0xFFFFFFFF;
   for (size_t i = 0; i < w * h; i++) {
     if(image[i * 4 + 3] == 0) {continue;}
     unsigned index = ColorIndex(&image[i * 4]) & 0xFFFFFF;
     if(prev != index) {
       opaque.insert(index);
       prev = index;
     }
   }

   // Try greyscale colors first, might also be easier to compress
   for (unsigned i = 0; i <= 0xFFFFFF; i += 0x010101) {
     if (!opaque.count(i)) {
       return i;
     }
   }

   // Find first unused color
   unsigned expected = 0;
   for(unsigned p : opaque) {
     if (p != expected) {
       return expected;
     }
     expected++;
   }

   // only reachable if every possible color is used, forego key in that case
   return 0xFF000000;
}

// Remove RGB information from pixels with alpha=0
static void LossyOptimizeTransparent(lodepng::State* inputstate, unsigned char* image,
                                     unsigned w, unsigned h, int filter) {
  std::unordered_set<unsigned> count;  // Color count, up to 257.

  // If true, means palette is possible so avoid using different RGB values for
  // the transparent color.
  CountColors(&count, image, w, h);
  unsigned long colors = count.size();
  bool palette_possible = colors <= 256;
  if(12 + colors * 4 > w * h / 2) palette_possible = false;

  // First check if we want to preserve potential color-key background color,
  // or instead use the last encountered RGB value all the time to save bytes.
  bool key_possible = true;
  for (size_t i = 0; i < w * h; i++) {
    if (image[i * 4 + 3] > 0 && image[i * 4 + 3] < 255) {
      key_possible = false;
      break;
    }
  }

  unsigned unused_color = 0xFF000000;
  if (key_possible && (unused_color = FindUnusedColor(image, w, h)) != 0xFF000000) {
    unsigned char* uc = (unsigned char*)&unused_color;
    unsigned char r = uc[0];
    unsigned char g = uc[1];
    unsigned char b = uc[2];

    for (size_t i = 0; i < w * h; i++) {
      if (image[i * 4 + 3] == 0) {
        image[i * 4] = r;
        image[i * 4 + 1] = g;
        image[i * 4 + 2] = b;
      }
    }
  } else {
    int pre = 0, pgr = 0, pbl = 0;

    if (!filter || palette_possible){
      for (size_t i = 0; i < w * h; i++) {
        // if alpha is 0, alter the RGB values to 0.
        if (image[i * 4 + 3] == 0) {
          image[i * 4] = 0;
          image[i * 4 + 1] = 0;
          image[i * 4 + 2] = 0;
        }
      }
    }
    else if (filter == 1){
      for (size_t i = 0; i < ((w << 2) * h); i += (w << 2)) {
        for (size_t j = 3; j < (w << 2); j += 4) {
          // if alpha is 0, set the RGB values to those of the pixel on the
          // left.
          if (image[i + j] == 0) {
            image[i + j - 3] = pre;
            image[i + j - 2] = pgr;
            image[i + j - 1] = pbl;
          } else {
            // Use the last encountered RGB value.
            pre = image[i + j - 3];
            pgr = image[i + j - 2];
            pbl = image[i + j - 1];
          }
        }
        if (w > 1) {
          for (size_t j = ((w - 2) << 2) + 3; j + 1 > 0; j -= 4) {
            // if alpha is 0, set the RGB values to those of the pixel on the
            // right.
            if (image[i + j] == 0) {
              image[i + j - 3] = pre;
              image[i + j - 2] = pgr;
              image[i + j - 1] = pbl;
            } else {
              // Use the last encountered RGB value.
              pre = image[i + j - 3];
              pgr = image[i + j - 2];
              pbl = image[i + j - 1];
            }
          }
        }
        pre = pgr = pbl = 0;   // reset to zero at each new line
      }

    }
    else if (filter == 2){
      for (size_t j = 3; j < (w << 2); j += 4) {
        // if alpha is 0, set the RGB values to zero (black), first line only.
        if (image[j] == 0) {
          image[j - 3] = 0;
          image[j - 2] = 0;
          image[j - 1] = 0;
        }
      }
      if (h > 1) {
        for (size_t j = 3; j < (w << 2); j += 4) {
          for (size_t i = (w << 2); i < ((w << 2) * h); i += (w << 2)) {
            // if alpha is 0, set the RGB values to those of the upper pixel.
            if (image[i + j] == 0) {
              image[i + j - 3] = image[i + j - 3 - (w << 2)];
              image[i + j - 2] = image[i + j - 2 - (w << 2)];
              image[i + j - 1] = image[i + j - 1 - (w << 2)];
            }
          }
          for (size_t i = (w << 2) * (h - 2); i + (w << 2) > 0; i -= (w << 2)) {
            // if alpha is 0, set the RGB values to those of the lower pixel.
            if (image[i + j] == 0) {
              image[i + j - 3] = image[i + j - 3 + (w << 2)];
              image[i + j - 2] = image[i + j - 2 + (w << 2)];
              image[i + j - 1] = image[i + j - 1 + (w << 2)];
            }
          }
        }
      }
    }
    else if (filter == 3){
      for (size_t j = 3; j < (w << 2); j += 4) {
        // if alpha is 0, set the RGB values to the half of those of the pixel
        // on the left, first line only.
        if (image[j] == 0) {
          pre = pre >> 1;
          pgr = pgr >> 1;
          pbl = pbl >> 1;
          image[j - 3] = pre;
          image[j - 2] = pgr;
          image[j - 1] = pbl;
        } else {
          pre = image[j - 3];
          pgr = image[j - 2];
          pbl = image[j - 1];
        }
      }
      if (h > 1) {
        for (size_t i = (w << 2); i < ((w << 2) * h); i += (w << 2)) {
          pre = pgr = pbl = 0;   // reset to zero at each new line
          for (size_t j = 3; j < (w << 2); j += 4) {
            // if alpha is 0, set the RGB values to the half of the sum of the
            // pixel on the left and the upper pixel.
            if (image[i + j] == 0) {
              pre = (pre + (int)image[i + j - (3 + (w << 2))]) >> 1;
              pgr = (pgr + (int)image[i + j - (2 + (w << 2))]) >> 1;
              pbl = (pbl + (int)image[i + j - (1 + (w << 2))]) >> 1;
              image[i + j - 3] = pre;
              image[i + j - 2] = pgr;
              image[i + j - 1] = pbl;
            } else {
              pre = image[i + j - 3];
              pgr = image[i + j - 2];
              pbl = image[i + j - 1];
            }
          }
        }
      }
    }
    else if (filter == 4){
      for (size_t j = 3; j < (w << 2); j += 4) {  // First line (border effects)
        // if alpha is 0, alter the RGB value to a possibly more efficient one.
        if (image[j] == 0) {
          image[j - 3] = pre;
          image[j - 2] = pgr;
          image[j - 1] = pbl;
        } else {
          pre = image[j - 3];
          pgr = image[j - 2];
          pbl = image[j - 1];
        }
      }
      if (h > 1) {
        int a, b, c, pa, pb, pc, p;
        for (size_t i = (w << 2); i < ((w << 2) * h); i += (w << 2)) {
          pre = pgr = pbl = 0;   // reset to zero at each new line
          for (size_t j = 3; j < (w << 2); j += 4) {
            // if alpha is 0, set the RGB values to the Paeth predictor.
            if (image[i + j] == 0) {
              if (j != 3) {  // not in first column
                a = pre;
                b = (int)image[i + j - (3 + (w << 2))];
                c = (int)image[i + j - (7 + (w << 2))];
                p = b - c;
                pc = a - c;
                pa = abs(p);
                pb = abs(pc);
                pc = abs(p + pc);
                pre = (pa <= pb && pa <=pc) ? a : (pb <= pc) ? b : c;

                a = pgr;
                b = (int)image[i + j - (2 + (w << 2))];
                c = (int)image[i + j - (6 + (w << 2))];
                p = b - c;
                pc = a - c;
                pa = abs(p);
                pb = abs(pc);
                pc = abs(p + pc);
                pgr = (pa <= pb && pa <=pc) ? a : (pb <= pc) ? b : c;

                a = pbl;
                b = (int)image[i + j - (1 + (w << 2))];
                c = (int)image[i + j - (5 + (w << 2))];
                p = b - c;
                pc = a - c;
                pa = abs(p);
                pb = abs(pc);
                pc = abs(p + pc);
                pbl = (pa <= pb && pa <=pc) ? a : (pb <= pc) ? b : c;

                image[i + j - 3] = pre;
                image[i + j - 2] = pgr;
                image[i + j - 1] = pbl;
              } else {
                // first column, set the RGB values to those of the upper pixel.
                pre = (int)image[i + j - (3 + (w << 2))];
                pgr = (int)image[i + j - (2 + (w << 2))];
                pbl = (int)image[i + j - (1 + (w << 2))];
                image[i + j - 3] = pre;
                image[i + j - 2] = pgr;
                image[i + j - 1] = pbl;
              }
            } else {
              pre = image[i + j - 3];
              pgr = image[i + j - 2];
              pbl = image[i + j - 1];
            }
          }
        }
      }
    }
  }
}

// Tries to optimize given a single PNG filter strategy.
// Returns 0 if ok, other value for error
/* Palette indices are arbitrary, so PNG filters, which predict a byte from
its neighbours, rarely help indexed images in their original order. After
reordering the palette so that colors that are often next to each other get
nearby indices (oxipng's luma, ezeng and battiato orders), filtering often
pays off. Each order is tried unfiltered and filtered, ranked by its zlib
size, and only the best one is encoded with zopfli.

Indexed rows are filtered by the smallest Shannon entropy per row (as
LFS_ENTROPY and oxipng do): on reordered palettes it gives 3-6% smaller
files than choosing filters with the deflate size estimator, which also
fails to rank filtered palette data. */
static void EntropyFilter(unsigned char* out, const unsigned char* in, unsigned w, unsigned h) {
  std::vector<unsigned char> cand(5 * (size_t)w);
  const unsigned char* prev = 0;
  for (unsigned y = 0; y < h; y++) {
    const unsigned char* row = in + (size_t)y * w;
    double best = 0;
    unsigned best_type = 0;
    for (unsigned t = 0; t < 5; t++) {
      unsigned char* c = &cand[t * (size_t)w];
      for (unsigned x = 0; x < w; x++) {
        int a = x ? row[x - 1] : 0, b = prev ? prev[x] : 0, d = (x && prev) ? prev[x - 1] : 0, pred;
        switch (t) {
          case 0: pred = 0; break;
          case 1: pred = a; break;
          case 2: pred = b; break;
          case 3: pred = (a + b) >> 1; break;
          default: {
            int pa = abs(b - d), pb = abs(a - d), pc = abs(a + b - 2 * d);
            pred = (pa <= pb && pa <= pc) ? a : (pb <= pc ? b : d);
          }
        }
        c[x] = (unsigned char)(row[x] - pred);
      }
      unsigned count[256] = {0};
      for (unsigned x = 0; x < w; x++) count[c[x]]++;
      double e = 0;
      for (unsigned i = 0; i < 256; i++) {
        if (count[i]) e -= count[i] * log2((double)count[i] / w);
      }
      if (t == 0 || e < best) {
        best = e;
        best_type = t;
      }
    }
    out[(size_t)y * (w + 1)] = (unsigned char)best_type;
    memcpy(&out[(size_t)y * (w + 1) + 1], &cand[best_type * (size_t)w], w);
    prev = row;
  }
}

/* Compressed size with zlib, which ranks filtered palette data much better
   than the deflate size estimator does. */
static size_t ZlibSize(const std::vector<unsigned char>& data) {
  z_stream stream;
  memset(&stream, 0, sizeof(stream));
  if (deflateInit2(&stream, 6, Z_DEFLATED, 15, 8, Z_FILTERED) != Z_OK) return data.size();
  stream.next_in = (z_const unsigned char*)data.data();
  stream.avail_in = (uInt)data.size();
  std::vector<unsigned char> buf(deflateBound(&stream, (uLong)data.size()));
  stream.next_out = buf.data();
  stream.avail_out = (uInt)buf.size();
  deflate(&stream, Z_FINISH);
  size_t size = stream.total_out;
  deflateEnd(&stream);
  return size;
}

static void TryPaletteOrders(const unsigned char* image, size_t imagesize, unsigned w, unsigned h,
                             lodepng::State& state, const LodePNGColorMode& ref, const LodePNGColorMode& input,
                             std::vector<unsigned char>* out) {
  const size_t n = ref.palettesize;
  const size_t pixels = (size_t)w * h;
  if (ref.colortype != LCT_PALETTE || n < 3 || n > 256 || pixels < 1024 || imagesize < pixels * 4) return;

  std::unordered_map<uint32_t, unsigned char> lookup;
  for (size_t i = 0; i < n; i++) {
    const unsigned char* c = &ref.palette[i * 4];
    lookup.emplace((uint32_t)c[0] | (uint32_t)c[1] << 8 | (uint32_t)c[2] << 16 | (uint32_t)c[3] << 24, (unsigned char)i);
  }
  std::vector<unsigned char> idx(pixels);
  for (size_t i = 0; i < pixels; i++) {
    const unsigned char* c = &image[i * 4];
    auto it = lookup.find((uint32_t)c[0] | (uint32_t)c[1] << 8 | (uint32_t)c[2] << 16 | (uint32_t)c[3] << 24);
    if (it == lookup.end()) return;
    idx[i] = it->second;
  }

  palette_order::CoOccurrence matrix(idx.data(), w, h, n);
  std::vector<std::vector<size_t> > orders;
  std::vector<size_t> identity(n);
  for (size_t i = 0; i < n; i++) identity[i] = i;
  orders.push_back(identity);
  orders.push_back(palette_order::Luma(ref.palette, n, idx.data(), w, h));
  orders.push_back(palette_order::Ezeng(matrix, 50, pixels));
  orders.push_back(palette_order::Battiato(matrix));
  /* The palette order of the input file, which may already be a good one. */
  if (input.colortype == LCT_PALETTE) {
    std::vector<size_t> order;
    std::vector<bool> used(n);
    for (size_t i = 0; i < input.palettesize; i++) {
      const unsigned char* c = &input.palette[i * 4];
      auto it = lookup.find((uint32_t)c[0] | (uint32_t)c[1] << 8 | (uint32_t)c[2] << 16 | (uint32_t)c[3] << 24);
      if (it != lookup.end() && !used[it->second]) {
        used[it->second] = true;
        order.push_back(it->second);
      }
    }
    for (size_t i = 0; i < n; i++) if (!used[i]) order.push_back(i);
    orders.push_back(order);
  }

  /* Rank on evenly spaced bands of rows, at most about 256K pixels, which
     keeps this cheap on large images. */
  const size_t kBand = 32, kMaxPixels = 262144;
  std::vector<unsigned> sample;
  if (pixels <= kMaxPixels) {
    for (unsigned y = 0; y < h; y++) sample.push_back(y);
  } else {
    size_t bands = std::max<size_t>(1, kMaxPixels / ((size_t)w * kBand));
    for (size_t b = 0; b < bands; b++) {
      size_t start = (size_t)h * b / bands;
      for (size_t y = start; y < std::min<size_t>(h, start + kBand); y++) sample.push_back((unsigned)y);
    }
  }
  const unsigned sh = (unsigned)sample.size();
  const size_t stride = (size_t)w + 1;
  std::vector<unsigned char> remapped((size_t)w * sh), rows(stride * sh), filtered(stride * sh);
  double base = 0, best = 0;
  size_t best_order = 0;
  bool best_filtered = false;
  for (size_t o = 0; o < orders.size(); o++) {
    const std::vector<size_t>& order = orders[o];
    if (order.size() != n) continue;
    unsigned char map[256];
    for (size_t i = 0; i < n; i++) map[order[i]] = (unsigned char)i;
    for (unsigned s = 0; s < sh; s++) {
      const unsigned char* src = &idx[(size_t)sample[s] * w];
      unsigned char* dst = &remapped[(size_t)s * w];
      for (unsigned x = 0; x < w; x++) dst[x] = map[src[x]];
      rows[s * stride] = 0;
      memcpy(&rows[s * stride + 1], dst, w);
    }
    double zero = (double)ZlibSize(rows);
    EntropyFilter(filtered.data(), remapped.data(), w, sh);
    double filt = (double)ZlibSize(filtered);
    if (o == 0) best = base = zero;
    if (zero < best) { best = zero; best_order = o; best_filtered = false; }
    if (filt < best) { best = filt; best_order = o; best_filtered = true; }
  }
  if (best_order == 0 && !best_filtered) return;
  /* Another encode is only worth it for a clear estimated gain. */
  if (best > base * 0.99) return;

  lodepng_color_mode_copy(&state.info_png.color, &ref);
  state.info_png.color.bitdepth = 8;
  for (size_t i = 0; i < n; i++) {
    memcpy(&state.info_png.color.palette[i * 4], &ref.palette[orders[best_order][i] * 4], 4);
  }
  state.encoder.auto_convert = 0;
  state.encoder.filter_strategy = best_filtered ? LFS_ENTROPY : LFS_ZERO;
  LodePNGPaletteSettings p;
  p.order = LPOS_NONE;
  std::vector<unsigned char> out2;
  unsigned e2 = lodepng::encode(out2, image, imagesize, w, h, state, p);
  if (!e2 && out2.size() < out->size()) {
    out->swap(out2);
  }
}

static unsigned TryOptimize(unsigned char* image, size_t imagesize, unsigned w, unsigned h, bool bit16, const lodepng::State& inputstate,
                            const ZopfliPNGOptions* png_options, std::vector<unsigned char>* out, int best_filter, std::vector<unsigned char> filters, unsigned palette_filter) {
  lodepng::State state;
  state.encoder.zlibsettings.custom_deflate = CustomPNGDeflate;
  state.encoder.zlibsettings.custom_context = png_options;
  state.encoder.clean_alpha = png_options->lossy_transparent;
  state.encoder.quiet = png_options->quiet;

  ZopfliOptions dummyoptions;
  ZopfliInitOptions(&dummyoptions, png_options->Mode, 0, 0);
  state.encoder.filter_style = dummyoptions.filter_style;
  state.encoder.text_compression = 0;
  if (bit16) {
    state.info_raw.bitdepth = 16;
  }

  state.encoder.filter_strategy = (LodePNGFilterStrategy)best_filter;
  if (best_filter == 6)
  {
    state.encoder.predefined_filters = &filters[0];
    state.encoder.auto_convert = 0;
    lodepng_color_mode_copy(&state.info_png.color, &inputstate.info_png.color);
  }

  LodePNGPaletteSettings p;
  p.order = LPOS_NONE;
  state.div = png_options->Mode == 2 ? 6 : png_options->Mode < 8 ? 3 : 2;
  unsigned error = lodepng::encode(*out, image, imagesize, w, h, state, p);
  LodePNGColorMode ref_color;

  // No need for palette sort or compressing without palette when reusing color.
  if (best_filter == 6 || error){
    return error;
  }
  lodepng_color_mode_init(&ref_color);
  lodepng_color_mode_copy(&ref_color, &state.out_mode);

  // Try different ways to sort palette
  if (!error && state.out_mode.colortype == LCT_PALETTE && palette_filter && state.out_mode.palettesize > 1) {
    p._first = 1;
    std::vector<unsigned char> out2;
    unsigned tries = 0;

    // TODO: Using (LodePNGPaletteOrderStrategy)3 (LodePNGPalettePriorityStrategy)0 (LodePNGPaletteTransparencyStrategy)0 (LodePNGPaletteDirectionStrategy)1 should be a better default search strategy.
    // Can also do both the order produced by lodepng and the one described above
    // TODO: The subset (LodePNGPaletteOrderStrategy) (0, 2, 3) (LodePNGPalettePriorityStrategy) (0,2) (LodePNGPaletteTransparencyStrategy)0 (LodePNGPaletteDirectionStrategy) 1 performs almost as good as trying all 120 filter strategies, change order in which we go through the different sortings and
    // consider removing the worse-performing options – always having LodePNGPaletteTransparencyStrategy 0 looks like a cheap win
    for (int k4 = 0; k4 < 4; k4++){
      p.order = (LodePNGPaletteOrderStrategy)k4;
      for (int k3 = 0; k3 < 5; k3++){
        p.priority = (LodePNGPalettePriorityStrategy)k3;
        for (int k2 = 0; k2 < 3; k2++){
          p.trans = (LodePNGPaletteTransparencyStrategy)k2;
          if (!lodepng_can_have_alpha(&state.out_mode)){
            k2 = 3;
          }

          for (int k1 = 0; k1 < 2; k1++){
            p.direction = (LodePNGPaletteDirectionStrategy)k1;

            lodepng_color_mode_cleanup(&state.out_mode);
            p._first += ((tries + 1) == palette_filter) << 1;
            lodepng::encode(out2, image, imagesize, w, h, state, p);
            p._first = 0;

            if (out2.size() < out->size() && !state.note){
              out->swap(out2);
            }
            out2.clear();
            if (++tries == palette_filter){
              k1 = k2 = k3 = k4 = 5;
            }
          }
        }
      }
    }
  }

  // For very small output, also try without palette, it may be smaller thanks
  // to no palette storage overhead.
  unsigned long testboth = out->size();
  if (!error && testboth < 3800 && w * h < 100000 && state.out_mode.colortype == LCT_PALETTE) {
    LodePNGColorMode& color = ref_color;
    unsigned ux = color.palettesize;
    int wh_ok = (ux + 2) * 390 + 370;
    int size_ok = (ux + 2) * 40;

    if ((wh_ok > w*h || ux > 170)
        && (size_ok > testboth || ux > 180)
        && (size_ok / 2 > testboth || ux < 24)
        && (png_options->Mode > 2 || (testboth < 3400 && w*h < 20000 &&(size_ok * 7/ 20 > testboth || ux < 64))))
    {
      std::vector<unsigned char> out2;
      state.encoder.auto_convert = 0;
      bool grey = true;
      unsigned has_alpha = lodepng_has_palette_alpha(&color);
      for (size_t i = 0; i < color.palettesize; i++) {
        if (color.palette[i * 4] != color.palette[i * 4 + 2]
            || color.palette[i * 4 + 1] != color.palette[i * 4 + 2]) {
          grey = false;
          break;
        }
      }
      if (grey) {
        state.info_png.color.colortype = has_alpha ? LCT_GREY_ALPHA : LCT_GREY;
      } else{
        state.info_png.color.colortype = has_alpha ? LCT_RGBA : LCT_RGB;
      }
      error = lodepng::encode(out2, image, imagesize, w, h, state, p);
      if (out2.size() < out->size()){
        out->swap(out2);
      }

    }
  }

  // Palette and grey images with fewer than 8 bits per pixel pack several
  // pixels into one byte, so a repeated pattern only matches LZ77 when it
  // starts at the same position within a byte. Stored with 8 bits per pixel
  // the image data is larger, but it often deflates to less.
  if (!error && ref_color.bitdepth < 8 && (ref_color.colortype == LCT_PALETTE || ref_color.colortype == LCT_GREY)) {
    std::vector<unsigned char> out2;
    state.encoder.auto_convert = 0;
    lodepng_color_mode_copy(&state.info_png.color, &ref_color);
    state.info_png.color.bitdepth = 8;
    LodePNGPaletteSettings p8;
    p8.order = LPOS_NONE;
    if (!lodepng::encode(out2, image, imagesize, w, h, state, p8) && out2.size() < out->size()) {
      out->swap(out2);
    }
  }

  if (!error && !bit16) {
    TryPaletteOrders(image, imagesize, w, h, state, ref_color, inputstate.info_png.color, out);
  }

  if(!error){
    lodepng_color_mode_cleanup(&ref_color);
    lodepng_color_mode_cleanup(&state.out_mode);
  }
  return error;
}

static unsigned ZopfliPNGOptimize(const char * Infile, const std::vector<unsigned char>& origpng, const ZopfliPNGOptions& png_options, std::vector<unsigned char>* resultpng, int best_filter,
                                  std::vector<unsigned char> filters, unsigned palette_filter) {
  unsigned char* image = 0;
  size_t imagesize = 0;
  unsigned w, h;
  lodepng::State inputstate;

  unsigned error = lodepng::decode(&image, imagesize, w, h, inputstate, origpng.data(), origpng.size());

  bool bit16 = false;  // Using 16-bit per channel raw image
  if (!error && inputstate.info_png.color.bitdepth == 16 && !png_options.lossy_8bit) {
    // Decode as 16-bit
    free(image);
    error = lodepng::decode(&image, imagesize, w, h, origpng.data(), origpng.size(), LCT_RGBA, 16);
    bit16 = true;
  }

  if (error) {
    fprintf(stderr, "%s decoding error %i: %s\n", Infile, error, lodepng_error_text(error));
    return error;
  }
  // If lossy_transparent, remove RGB information from pixels with alpha=0
  if (png_options.lossy_transparent && !bit16 && lodepng_can_have_alpha(&inputstate.info_png.color)) {
    LossyOptimizeTransparent(&inputstate, image, w, h, best_filter < 5 ? best_filter : 1);
  }

  error = TryOptimize(image, imagesize, w, h, bit16, inputstate, &png_options, resultpng, best_filter, filters, palette_filter);
  free(image);
  if (error) {
    fprintf(stderr, "%s encoding error %u: %s\n", Infile, error, lodepng_error_text(error));
    return error;
  }
  if (!png_options.strip) {
    std::vector<std::string> names[3];
    std::vector<std::vector<unsigned char> > chunks[3];
    lodepng::getChunks(names, chunks, origpng);
    lodepng::insertChunks(*resultpng, chunks);
  }
  return 0;
}

int Zopflipng(bool strip, const char * Infile, bool strict, unsigned Mode, int filter, unsigned multithreading, unsigned quiet) {
  ZopfliPNGOptions png_options;
  png_options.Mode = Mode;
  png_options.multithreading = multithreading;
  png_options.quiet = quiet;
  unsigned palette_filter = (filter & 0xFF00) >> 8;
  filter &= 0xFF;
  png_options.lossy_transparent = !strict && filter != 6;
  png_options.strip = strip;
  std::vector<unsigned char> origpng;

  std::vector<unsigned char> filters;
  unsigned error = lodepng::load_file(origpng, Infile);
  if (error) {
    fprintf(stderr, "Could not load PNG %s\n", Infile);
    return -1;
  }
  if (filter == 6){
    lodepng::getFilterTypes(filters, origpng);
    if(!filters.size()){
      fprintf(stderr, "%s: Could not load PNG filters\n", Infile);
      return -1;
    }
  }
  std::vector<unsigned char> resultpng;
  if (ZopfliPNGOptimize(Infile, origpng, png_options, &resultpng, filter, filters, palette_filter)) {return -1;}
  if (resultpng.size() >= origpng.size()) {return 1;}
  if (lodepng::save_file(resultpng, Infile) != 0) {
    fprintf(stderr, "Failed to write to file %s\n", Infile);
    return -1;
  }
  return 0;
}
