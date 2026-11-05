/*
Cheap estimate of the deflate size of filtered PNG scanlines, used to choose
the filter of each row (LFS_ESTIMATE) without running zlib.

It runs a small greedy LZ77 parse (hash chains of depth 16, plus candidates at
the distance of one row and of the previous match) over the filtered data and
prices the resulting symbols with an entropy model that tracks deflate blocks.
A row is scored by appending it to the parse of all rows chosen so far; the
state of the best candidate is kept and committed once the row is decided, so
the cost is close to linear in the image size.

From clevert-app/ect (Apache License 2.0), src/ectpng.cpp, commits 96a9ac6
"Add PNG filter strategy 12, a cheap estimator, to replace try-deflate
strategy 11" and fb5d564 "Better PNG filter estimator", by kkocdko.
*/

#ifndef ECT_FILTER_ESTIMATOR_H
#define ECT_FILTER_ESTIMATOR_H

#include <algorithm>
#include <array>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <vector>

#if defined(_MSC_VER) && !defined(__clang__)
#include <intrin.h>
#endif

namespace filter_estimator {

/* Index of the highest set bit; v must not be 0. */
static inline unsigned HighBit32(uint32_t v) {
#if defined(__GNUC__) || defined(__clang__)
  return 31u - (unsigned)__builtin_clz(v);
#elif defined(_MSC_VER)
  unsigned long index;
  _BitScanReverse(&index, v);
  return (unsigned)index;
#else
  unsigned r = 0;
  while (v >>= 1) r++;
  return r;
#endif
}

/* Number of equal leading bytes in memory order given x = a ^ b != 0. */
static inline unsigned FirstDiffBytes(uint64_t x) {
#if defined(__BYTE_ORDER__) && __BYTE_ORDER__ == __ORDER_BIG_ENDIAN__
  return (unsigned)__builtin_clzll(x) / 8;
#elif defined(__GNUC__) || defined(__clang__)
  return (unsigned)__builtin_ctzll(x) / 8;
#elif defined(_MSC_VER) && (defined(_M_X64) || defined(_M_ARM64))
  unsigned long index;
  _BitScanForward64(&index, x);
  return (unsigned)index / 8;
#else
  unsigned n = 0;
  while (!(x & 0xFF)) { x >>= 8; n++; }
  return n;
#endif
}

class FilterEstimator {
  static constexpr size_t hash_size = 16384;
  static constexpr size_t short_hash_size = 4096;
  static constexpr size_t window_size = 32768;
  static constexpr size_t lookahead = 262;

  struct Counts {
    std::array<unsigned, 286> literal{};
    std::array<unsigned, 30> distance{};
    unsigned symbols = 0, extra_bits = 0, fixed_bits = 0;
    unsigned bytes = 0, recent_distance = 0;
    double completed_bits = 0;

    Counts() { literal[256] = 1; }

    static const std::array<double, 16385>& EntropyBits() {
      static const std::array<double, 16385> values = [] {
        std::array<double, 16385> v{};
        for (size_t count = 1; count < v.size(); count++)
          v[count] = count * std::log2(double(count));
        return v;
      }();
      return values;
    }

    template <size_t N> static double Entropy(const std::array<unsigned, N>& frequencies) {
      const std::array<double, 16385>& entropy_bits = EntropyBits();
      unsigned total = 0, used = 0, maximum = 0;
      double bits = 0;
      for (unsigned count : frequencies) {
        total += count;
        used += count != 0;
        maximum = std::max(maximum, count);
        bits -= entropy_bits[count];
      }
      if (maximum > total / 2)
        return total + entropy_bits[total - maximum] + bits + entropy_bits[maximum] + used * 3;
      return total ? std::max(double(total), entropy_bits[total] + bits) + used * 3 : 0;
    }

    double cost() const {
      return completed_bits + std::min(double(fixed_bits + 10),
                                       Entropy(literal) + Entropy(distance) + extra_bits + 60);
    }

    template <bool whole> void finish_symbol(unsigned size) {
      if (!whole)
        bytes += size;
      if (++symbols == 16383 || (!whole && bytes >= 131072)) {
        double bits = cost();
        unsigned saved_distance = recent_distance;
        *this = Counts();
        recent_distance = saved_distance;
        completed_bits = bits;
      }
    }

    template <bool whole> void add_literal(unsigned value) {
      literal[value]++;
      fixed_bits += value < 144 ? 8 : 9;
      finish_symbol<whole>(1);
    }

    template <bool whole> void add_match(unsigned length, unsigned offset) {
      if (!whole)
        recent_distance = offset;
      unsigned length_code, length_extra = 0;
      if (length <= 10)
        length_code = length + 254;
      else if (length == 258)
        length_code = 285;
      else {
        length_extra = HighBit32(length - 3) - 2;
        length_code = 257 + 4 * (length_extra + 1) + (((length - 3) >> length_extra) & 3);
      }
      unsigned distance_code = offset - 1, distance_extra = 0;
      if (offset > 4) {
        distance_extra = HighBit32(offset - 1) - 1;
        distance_code = 2 * (distance_extra + 1) + (((offset - 1) >> distance_extra) & 1);
      }
      literal[length_code]++;
      distance[distance_code]++;
      extra_bits += length_extra + distance_extra;
      fixed_bits += (length_code <= 279 ? 7 : 8) + 5 + length_extra + distance_extra;
      finish_symbol<whole>(length);
    }
  };

  std::vector<size_t> heads, trial_heads, best_heads;
  std::vector<uint16_t> previous, trial_previous, best_previous;
  std::vector<size_t> short_heads, trial_short_heads, best_short_heads;
  Counts counts, candidate_counts, best_counts;
  size_t next_position = 0, candidate_end = 0, best_end = 0, row_stride;

  static size_t hash(const uint8_t* data) {
    uint32_t value;
    memcpy(&value, data, sizeof(value));
    return (value * 0x1e35a7bdU) >> 18;
  }

  static size_t short_hash(const uint8_t* data) {
    uint32_t value = uint32_t(data[0]) | uint32_t(data[1]) << 8 | uint32_t(data[2]) << 16;
    return (value * 0x1e35a7bdU) >> 20;
  }

  template <bool whole> double evaluate(const uint8_t* data, size_t end) {
    if (!whole) {
      trial_heads = heads;
      trial_short_heads = short_heads;
    }
    std::vector<size_t>& active_heads = whole ? heads : trial_heads;
    std::vector<size_t>& active_short_heads = whole ? short_heads : trial_short_heads;
    std::vector<uint16_t>& active_previous = whole ? previous : trial_previous;
    const size_t base = next_position;
    auto previous_index = [&](size_t position) {
      return whole ? position & (window_size - 1) : position - base;
    };
    Counts trial = counts;
    size_t first = next_position;
    candidate_counts = counts;
    candidate_end = first;
    bool tail = false;
    struct UndoEntry {
      size_t index, short_index, head, short_head;
    };
    std::array<UndoEntry, lookahead> undo;
    size_t undo_count = 0;
    struct MatchHeads {
      size_t match, short_match;
    };
    auto insert = [&](size_t position) {
      size_t index = hash(data + position);
      size_t short_index = short_hash(data + position);
      size_t match = active_heads[index];
      size_t short_match = active_short_heads[short_index];
      if (tail)
        undo[undo_count++] = {index, short_index, match, short_match};
      active_previous[previous_index(position)] =
          (uint16_t)(match ? std::min(position + 1 - match, window_size + 1) : 0);
      active_heads[index] = position + 1;
      active_short_heads[short_index] = position + 1;
      return MatchHeads{match, short_match};
    };
    size_t position = first;
    while (position < end) {
      if (!whole && !tail && end - position < lookahead) {
        candidate_counts = trial;
        candidate_end = position;
        tail = true;
      }
      MatchHeads matches{0, 0};
      if (end - position >= 4)
        matches = insert(position);
      else if (end - position >= 3)
        matches.short_match = active_short_heads[short_hash(data + position)];
      size_t match = matches.match;
      size_t best_length = 3, best_distance = 0;
      size_t limit = std::min(size_t(258), end - position);
      auto match_length = [&](size_t offset) {
        size_t length = 4;
        while (length + 8 <= limit) {
          uint64_t left, right;
          memcpy(&left, data + offset + length, 8);
          memcpy(&right, data + position + length, 8);
          if (left != right) {
            length += FirstDiffBytes(left ^ right);
            break;
          }
          length += 8;
        }
        while (length < limit && data[offset + length] == data[position + length])
          length++;
        return length;
      };
      unsigned probes = 16;
      for (unsigned depth = 0; match && depth < probes; depth++) {
        size_t offset = match - 1;
        if (position - offset > window_size)
          break;
        if (data[offset + best_length] == data[position + best_length] &&
            memcmp(data + offset, data + position, 4) == 0) {
          size_t length = match_length(offset);
          if (length > best_length) {
            best_length = length;
            best_distance = position - offset;
            if (length >= 32)
              probes = 8;
            if (length == limit)
              break;
          }
        }
        size_t step = whole || offset >= first ? active_previous[previous_index(offset)]
                                               : previous[offset & (window_size - 1)];
        match = step && step <= offset ? offset - step + 1 : 0;
      }
      if (!whole) {
        const size_t distances[2] = {row_stride, size_t(trial.recent_distance)};
        for (size_t distance : distances) {
          if (!distance || distance > window_size || position < distance ||
              limit < 4 || distance == best_distance ||
              (best_length == limit && distance >= best_distance))
            continue;
          size_t offset = position - distance;
          if (data[offset + best_length - 1] == data[position + best_length - 1] &&
              memcmp(data + offset, data + position, 4) == 0) {
            size_t length = match_length(offset);
            if (length > best_length ||
                (length == best_length && distance < best_distance)) {
              best_length = length;
              best_distance = distance;
            }
          }
        }
      }
      if (!best_distance && matches.short_match && position - (matches.short_match - 1) < 8192 &&
          memcmp(data + position, data + matches.short_match - 1, 3) == 0) {
        best_length = 3;
        best_distance = position - (matches.short_match - 1);
      }
      if (best_distance) {
        trial.template add_match<whole>((unsigned)best_length, (unsigned)best_distance);
        size_t stop = std::min(position + best_length, end - 3);
        uint32_t prefix = 0;
        if (stop - position > 16)
          memcpy(&prefix, data + position, sizeof(prefix));
        if (stop - position > 16 && prefix == data[position] * 0x01010101U &&
            memcmp(data + position, data + position + 1, stop - position + 2) == 0) {
          size_t index = hash(data + position);
          size_t short_index = short_hash(data + position);
          if (tail)
            undo[undo_count++] = {index, short_index, active_heads[index],
                                  active_short_heads[short_index]};
          for (size_t inserted = position + 1; inserted < stop; inserted++)
            active_previous[previous_index(inserted)] = 1;
          active_heads[index] = stop;
          active_short_heads[short_index] = stop;
        } else {
          for (size_t inserted = position + 1; inserted < stop; inserted++)
            insert(inserted);
        }
        position += best_length;
      } else {
        trial.template add_literal<whole>(data[position]);
        position++;
      }
    }
    while (undo_count) {
      const UndoEntry& entry = undo[--undo_count];
      active_heads[entry.index] = entry.head;
      active_short_heads[entry.short_index] = entry.short_head;
    }
    return trial.cost();
  }

public:
  explicit FilterEstimator(size_t row_size, size_t stride_hint = 0)
      : heads(row_size ? hash_size : 0), trial_heads(heads.size()), best_heads(heads.size()),
        previous(row_size ? window_size : 0), trial_previous(row_size ? row_size + lookahead : 0),
        best_previous(trial_previous.size()), short_heads(row_size ? short_hash_size : 0),
        trial_short_heads(short_heads.size()), best_short_heads(short_heads.size()),
        row_stride(stride_hint ? stride_hint : row_size) {}

  /* Estimated bits of data[0, end) given the rows committed so far, which must
     be a prefix of data. At most row_size bytes may follow that prefix. */
  double score(const uint8_t* data, size_t end) { return evaluate<false>(data, end); }

  /* Estimated bits of a whole buffer. */
  static double estimate(const uint8_t* data, size_t size) {
    FilterEstimator estimator(0);
    estimator.heads.resize(hash_size);
    estimator.short_heads.resize(short_hash_size);
    estimator.previous.resize(window_size);
    return estimator.evaluate<true>(data, size);
  }

  /* Remembers the state of the last score() as the best candidate. */
  void keep() {
    best_heads.swap(trial_heads);
    best_previous.swap(trial_previous);
    best_short_heads.swap(trial_short_heads);
    best_counts = candidate_counts;
    best_end = candidate_end;
  }

  /* Makes the kept candidate part of the committed prefix. */
  void commit() {
    heads.swap(best_heads);
    short_heads.swap(best_short_heads);
    counts = best_counts;
    size_t oldest = best_end > window_size ? best_end - window_size : 0;
    for (size_t inserted = std::max(next_position, oldest); inserted < best_end; inserted++)
      previous[inserted & (window_size - 1)] = best_previous[inserted - next_position];
    next_position = best_end;
  }
};

}  // namespace filter_estimator

#endif
