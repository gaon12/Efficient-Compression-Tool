/* Palette orderings for indexed PNG images.

Ported from oxipng (src/reduction/palette.rs, https://github.com/oxipng/oxipng),
Copyright (c) 2016 Joshua Holmer and the oxipng contributors, MIT License:

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

Each function returns a remapping: remapping[new index] = old index. Indices
that are next to each other in the image get nearby values, so that PNG
filters (which predict a byte from its neighbours) produce small residuals. */

#ifndef ECT_PALETTE_ORDER_H
#define ECT_PALETTE_ORDER_H

#include <algorithm>
#include <cstdint>
#include <numeric>
#include <tuple>
#include <vector>

namespace palette_order {

/* How often each pair of colors is adjacent (left or above), symmetric. */
struct CoOccurrence {
  size_t n;
  std::vector<uint32_t> data;
  std::vector<std::tuple<unsigned, unsigned, uint32_t> > edges;  // sorted by weight, descending

  CoOccurrence(const unsigned char* idx, unsigned w, unsigned h, size_t colors) : n(colors), data(colors * colors) {
    for (unsigned y = 0; y < h; y++) {
      const unsigned char* row = idx + (size_t)y * w;
      const unsigned char* above = y ? row - w : 0;
      for (unsigned x = 0; x < w; x++) {
        if (x) data[row[x - 1] * n + row[x]]++;
        if (above) data[above[x] * n + row[x]]++;
      }
    }
    for (size_t i = 0; i < n; i++) {
      for (size_t j = 0; j <= i; j++) {
        uint32_t s = data[i * n + j] + (i != j ? data[j * n + i] : 0);
        data[i * n + j] = data[j * n + i] = s;
      }
    }
    for (size_t i = 0; i < n; i++) {
      for (size_t j = 0; j < i; j++) {
        if (data[i * n + j]) edges.emplace_back((unsigned)j, (unsigned)i, data[i * n + j]);
      }
    }
    std::stable_sort(edges.begin(), edges.end(),
                     [](const std::tuple<unsigned, unsigned, uint32_t>& a, const std::tuple<unsigned, unsigned, uint32_t>& b) {
                       return std::get<2>(a) > std::get<2>(b);
                     });
  }
  const uint32_t* row(size_t i) const { return &data[i * n]; }
  /* The most frequent color and its approximate pixel count. */
  std::pair<size_t, uint64_t> mostPopular() const {
    std::pair<size_t, uint64_t> best(0, 0);
    for (size_t i = 0; i < n; i++) {
      uint64_t s = 0;
      for (size_t j = 0; j < n; j++) s += row(i)[j];
      if (s > best.second) best = std::make_pair(i, s);
    }
    best.second /= 4;
    return best;
  }
};

/* Ascending alpha (7 high bits), then descending luma; the most popular edge
   color (next to the filter bytes) goes first when there is a clear winner. */
inline std::vector<size_t> Luma(const unsigned char* palette_rgba, size_t n, const unsigned char* idx, unsigned w, unsigned h) {
  std::vector<uint32_t> counts(256);
  for (unsigned y = 0; y < h; y++) {
    counts[idx[(size_t)y * w]]++;
    counts[idx[(size_t)y * w + w - 1]]++;
  }
  size_t top = 0;
  for (size_t i = 1; i < n; i++) if (counts[i] > counts[top]) top = i;
  bool clear = std::count(counts.begin(), counts.begin() + n, counts[top]) == 1;

  std::vector<size_t> order(n);
  std::iota(order.begin(), order.end(), 0);
  if (clear) order.erase(order.begin() + top);
  auto key = [&](size_t i) {
    const unsigned char* c = palette_rgba + i * 4;
    int a = c[3];
    return ((a & 0xFE) << 18) + (a & 1) - c[0] * 299 - c[1] * 587 - c[2] * 114;
  };
  std::stable_sort(order.begin(), order.end(), [&](size_t a, size_t b) { return key(a) < key(b); });
  if (clear) order.insert(order.begin(), top);
  return order;
}

inline void MostPopularFirst(std::vector<size_t>& r, const CoOccurrence& m, uint64_t min) {
  std::pair<size_t, uint64_t> mp = m.mostPopular();
  if (mp.second < min) return;
  size_t first = std::find(r.begin(), r.end(), mp.first) - r.begin();
  if (first >= r.size() / 2) {
    std::reverse(r.begin(), r.end());
    std::rotate(r.rbegin(), r.rbegin() + (first + 1) % r.size(), r.rend());
  } else {
    std::rotate(r.begin(), r.begin() + first, r.end());
  }
}

/* Swap pairs at most max_dist apart while that lowers the total
   weight * distance cost. */
inline void PairwiseSwap(std::vector<size_t>& r, const CoOccurrence& m, size_t max_dist) {
  const size_t n = r.size();
  unsigned swaps = 2;
  while (swaps >= 2) {
    swaps = 0;
    for (size_t a = 0; a + 1 < n; a++) {
      for (size_t b = a + 1; b < std::min(n, a + max_dist + 1); b++) {
        const uint32_t* ra = m.row(r[a]);
        const uint32_t* rb = m.row(r[b]);
        int64_t delta = 0, dist = (int64_t)(b - a);
        for (size_t i = 0; i < a; i++) delta += ((int64_t)ra[r[i]] - rb[r[i]]) * dist;
        for (size_t i = a + 1; i < b; i++) delta += ((int64_t)ra[r[i]] - rb[r[i]]) * ((int64_t)(a + b) - 2 * (int64_t)i);
        for (size_t i = b + 1; i < n; i++) delta -= ((int64_t)ra[r[i]] - rb[r[i]]) * dist;
        if (delta < 0) {
          std::swap(r[a], r[b]);
          swaps++;
        }
      }
    }
  }
}

/* Move single colors to the position where they lower the total weight *
   distance cost most, until no move helps (not in oxipng). Moving x one step
   past its neighbour y changes the cost by (L_x - L_y) - (R_x - R_y) when x
   is on the left, where L and R sum the weights to the colors left and right
   of the pair, so every target position of x is evaluated in O(n). */
inline void InsertionRefine(std::vector<size_t>& r, const CoOccurrence& m, unsigned max_passes) {
  const size_t n = r.size();
  std::vector<int64_t> left(n), total(n), pre(n + 1);
  auto recompute = [&]() {
    for (size_t p = 0; p < n; p++) {
      const uint32_t* row = m.row(r[p]);
      int64_t l = 0, t = 0;
      for (size_t q = 0; q < n; q++) {
        if (q == p) continue;
        t += row[r[q]];
        if (q < p) l += row[r[q]];
      }
      left[p] = l;
      total[p] = t;
    }
  };
  recompute();
  for (unsigned pass = 0; pass < max_passes; pass++) {
    bool moved = false;
    for (size_t a = 0; a < n; a++) {
      const size_t x = r[a];
      const uint32_t* wx = m.row(x);
      pre[0] = 0;
      for (size_t q = 0; q < n; q++) pre[q + 1] = pre[q] + (q == a ? 0 : wx[r[q]]);
      int64_t delta = 0, best = 0;
      size_t target = a;
      for (size_t q = a + 1; q < n; q++) {  // x moves right past r[q]
        int64_t wxy = wx[r[q]];
        int64_t lx = pre[q], rx = total[a] - lx - wxy;
        int64_t ly = left[q] - wxy, ry = total[q] - left[q];
        delta += (lx - ly) - (rx - ry);
        if (delta < best) { best = delta; target = q; }
      }
      delta = 0;
      for (size_t q = a; q-- > 0;) {  // x moves left past r[q]
        int64_t wxy = wx[r[q]];
        int64_t lx = pre[q], rx = total[a] - lx - wxy;
        int64_t ly = left[q], ry = total[q] - left[q] - wxy;
        delta += (ly - lx) - (ry - rx);
        if (delta < best) { best = delta; target = q; }
      }
      if (target != a) {
        if (target > a) std::rotate(r.begin() + a, r.begin() + a + 1, r.begin() + target + 1);
        else std::rotate(r.begin() + target, r.begin() + a, r.begin() + a + 1);
        recompute();
        moved = true;
      }
    }
    if (!moved) break;
  }
}

/* Zeng's reindexing (Pinho et al.), inserting each color where it adds the
   least cost instead of only at the ends, then refined by pairwise swaps and
   single color moves. */
inline std::vector<size_t> Ezeng(const CoOccurrence& m, size_t max_swap_dist, size_t pixels) {
  std::vector<size_t> r;
  if (m.edges.empty()) return r;
  r.push_back(std::get<0>(m.edges[0]));
  r.push_back(std::get<1>(m.edges[0]));
  std::vector<std::pair<size_t, uint64_t> > sums;
  size_t best_pos = 0;
  std::pair<size_t, uint64_t> best(0, 0);
  for (size_t i = 0; i < m.n; i++) {
    if (i == r[0] || i == r[1]) continue;
    std::pair<size_t, uint64_t> s(i, (uint64_t)m.row(i)[r[0]] + m.row(i)[r[1]]);
    if (s.second > best.second) {
      best_pos = sums.size();
      best = s;
    }
    sums.push_back(s);
  }
  while (!sums.empty()) {
    const size_t bi = best.first;
    const uint32_t* brow = m.row(bi);
    const size_t len = r.size();
    size_t pos = 0;
    int64_t best_cost = INT64_MAX, cross = 0;
    for (size_t p = 0; p <= len; p++) {
      int64_t cost = 0;
      for (size_t k = 0; k < len; k++) cost += (int64_t)brow[r[k]] * (int64_t)(k < p ? p - k : k + 1 - p);
      if (cost + cross < best_cost) {
        best_cost = cost + cross;
        pos = p;
      }
      if (p < len) {
        const uint32_t* rp = m.row(r[p]);
        for (size_t q = p + 1; q < len; q++) cross += rp[r[q]];
        for (size_t q = 0; q < p; q++) cross -= rp[r[q]];
      }
    }
    r.insert(r.begin() + pos, bi);
    sums[best_pos] = sums.back();
    sums.pop_back();
    best_pos = 0;
    best = std::make_pair(0, 0);
    for (size_t i = 0; i < sums.size(); i++) {
      sums[i].second += brow[sums[i].first];
      if (sums[i].second > best.second) {
        best_pos = i;
        best = sums[i];
      }
    }
  }
  PairwiseSwap(r, m, max_swap_dist);
  InsertionRefine(r, m, 50);
  PairwiseSwap(r, m, max_swap_dist);
  MostPopularFirst(r, m, pixels * 3 / 20);
  return r;
}

/* Approximate traveling salesman path (Battiato et al.): join the heaviest
   edges into chains. */
inline std::vector<size_t> Battiato(const CoOccurrence& m) {
  std::vector<std::vector<size_t> > chains;
  std::vector<std::pair<int, size_t> > vx(m.n, std::make_pair(0, 0));  // 0 unvisited, 1 chain end, 2 inside
  for (const auto& e : m.edges) {
    size_t i = std::get<0>(e), j = std::get<1>(e);
    std::pair<int, size_t> vi = vx[i], vj = vx[j];
    if (vi.first == 0 && vj.first == 0) {
      vx[i] = vx[j] = std::make_pair(1, chains.size());
      chains.push_back(std::vector<size_t>{i, j});
    } else if (vi.first == 0 && vj.first == 1) {
      vx[i] = std::make_pair(1, vj.second);
      vx[j].first = 2;
      std::vector<size_t>& c = chains[vj.second];
      if (c[0] == j) c.insert(c.begin(), i); else c.push_back(i);
    } else if (vi.first == 1 && vj.first == 0) {
      vx[j] = std::make_pair(1, vi.second);
      vx[i].first = 2;
      std::vector<size_t>& c = chains[vi.second];
      if (c[0] == i) c.insert(c.begin(), j); else c.push_back(j);
    } else if (vi.first == 1 && vj.first == 1 && vi.second != vj.second) {
      vx[i].first = vx[j].first = 2;
      size_t a = vi.second < vj.second ? i : j, b = vi.second < vj.second ? j : i;
      size_t ca = vx[a].second, cb = vx[b].second;
      std::vector<size_t> chainb;
      chainb.swap(chains[cb]);
      for (size_t v : chainb) vx[v].second = ca;
      std::vector<size_t>& chaina = chains[ca];
      if (chaina[0] == a && chainb[0] == b) {
        for (size_t v : chainb) chaina.insert(chaina.begin(), v);
      } else if (chaina[0] == a) {
        chaina.insert(chaina.begin(), chainb.begin(), chainb.end());
      } else if (chainb[0] == b) {
        chaina.insert(chaina.end(), chainb.begin(), chainb.end());
      } else {
        chaina.insert(chaina.end(), chainb.rbegin(), chainb.rend());
      }
    }
    if (chains[0].size() == m.n) break;
  }
  std::vector<size_t> r;
  for (const auto& c : chains) r.insert(r.end(), c.begin(), c.end());
  for (size_t i = 0; i < m.n; i++) if (vx[i].first == 0) r.push_back(i);
  MostPopularFirst(r, m, 0);
  return r;
}

}  // namespace palette_order

#endif
