package ect

import (
	"bytes"
	"encoding/binary"
	"errors"
	"hash/crc32"
)

var pngSignature = []byte{0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n'}

// pngChunk is one chunk of a PNG stream (length and CRC are recomputed when
// writing).
type pngChunk struct {
	typ  string
	data []byte
}

var errBadPNGChunks = errors.New("malformed PNG chunk structure")

// parsePNGChunks splits a PNG stream into chunks, stopping after IEND.
func parsePNGChunks(b []byte) ([]pngChunk, error) {
	if !bytes.HasPrefix(b, pngSignature) {
		return nil, errBadPNGChunks
	}
	var chunks []pngChunk
	pos := len(pngSignature)
	for pos+12 <= len(b) {
		n := int(binary.BigEndian.Uint32(b[pos:]))
		if n < 0 || n > len(b)-pos-12 {
			return nil, errBadPNGChunks
		}
		c := pngChunk{typ: string(b[pos+4 : pos+8]), data: b[pos+8 : pos+8+n]}
		chunks = append(chunks, c)
		pos += 12 + n
		if c.typ == "IEND" {
			return chunks, nil
		}
	}
	return nil, errBadPNGChunks
}

func writePNGChunks(chunks []pngChunk) []byte {
	var out bytes.Buffer
	out.Write(pngSignature)
	var hdr [8]byte
	for _, c := range chunks {
		binary.BigEndian.PutUint32(hdr[:4], uint32(len(c.data)))
		copy(hdr[4:], c.typ)
		out.Write(hdr[:])
		out.Write(c.data)
		crc := crc32.NewIEEE()
		crc.Write(hdr[4:])
		crc.Write(c.data)
		_ = binary.Write(&out, binary.BigEndian, crc.Sum32())
	}
	return out.Bytes()
}

// findPNGChunk returns the first chunk of the given type.
func findPNGChunk(chunks []pngChunk, typ string) (pngChunk, bool) {
	for _, c := range chunks {
		if c.typ == typ {
			return c, true
		}
	}
	return pngChunk{}, false
}

// Ancillary chunks that must precede PLTE (and IDAT); they are placed right
// after IHDR.
var pngEarlyChunks = map[string]bool{
	"cHRM": true, "gAMA": true, "iCCP": true, "sBIT": true, "sRGB": true,
	"cICP": true, "mDCV": true, "cLLI": true,
}

// Chunks whose content depends on the color type / bit depth / palette and
// that are only valid if the encoder kept those unchanged.
var pngColorDependentChunks = map[string]bool{"bKGD": true, "sBIT": true, "hIST": true}

// Ancillary chunks defined by the PNG specification; any other (private)
// chunk is copied only if its "safe to copy" bit is set, as the
// specification requires once the image data has been rewritten.
var pngKnownAncillary = map[string]bool{
	"cHRM": true, "gAMA": true, "iCCP": true, "sBIT": true, "sRGB": true,
	"cICP": true, "mDCV": true, "cLLI": true, "bKGD": true, "hIST": true,
	"pHYs": true, "sPLT": true, "eXIf": true, "tIME": true, "tEXt": true,
	"zTXt": true, "iTXt": true, "oFFs": true, "pCAL": true, "sCAL": true,
	"sTER": true,
}

// copyPNGMetadata returns `encoded` with the ancillary chunks of `original`
// re-inserted (the Go encoder writes only IHDR, PLTE, tRNS, IDAT and IEND).
// With strip set, metadata is dropped instead. Chunk order rules are kept:
// color-space chunks go before PLTE, everything that preceded IDAT in the
// original goes before IDAT, and chunks that followed IDAT go before IEND.
func copyPNGMetadata(original, encoded []byte, strip bool) ([]byte, error) {
	if strip {
		return encoded, nil
	}
	orig, err := parsePNGChunks(original)
	if err != nil {
		return nil, err
	}
	enc, err := parsePNGChunks(encoded)
	if err != nil {
		return nil, err
	}
	origHdr, _ := findPNGChunk(orig, "IHDR")
	encHdr, _ := findPNGChunk(enc, "IHDR")
	// Width, height, bit depth and color type (interlacing may differ).
	sameFormat := len(origHdr.data) >= 10 && len(encHdr.data) >= 10 && bytes.Equal(origHdr.data[:10], encHdr.data[:10])

	var early, beforeIDAT, afterIDAT []pngChunk
	seenIDAT := false
	for _, c := range orig {
		switch c.typ {
		case "IHDR", "PLTE", "tRNS", "IEND":
			continue
		case "IDAT":
			seenIDAT = true
			continue
		}
		if len(c.typ) != 4 || c.typ[0]&0x20 == 0 {
			continue // critical chunk the encoder regenerates (or unknown)
		}
		if !pngKnownAncillary[c.typ] && c.typ[3]&0x20 == 0 {
			continue // unknown and not safe to copy
		}
		if pngColorDependentChunks[c.typ] && !sameFormat {
			continue
		}
		switch {
		case pngEarlyChunks[c.typ]:
			early = append(early, c)
		case seenIDAT:
			afterIDAT = append(afterIDAT, c)
		default:
			beforeIDAT = append(beforeIDAT, c)
		}
	}
	if len(early)+len(beforeIDAT)+len(afterIDAT) == 0 {
		return encoded, nil
	}

	out := make([]pngChunk, 0, len(enc)+len(early)+len(beforeIDAT)+len(afterIDAT))
	insertedBefore := false
	for _, c := range enc {
		switch c.typ {
		case "IHDR":
			out = append(out, c)
			out = append(out, early...)
			continue
		case "IDAT":
			if !insertedBefore {
				out = append(out, beforeIDAT...)
				insertedBefore = true
			}
		case "IEND":
			out = append(out, afterIDAT...)
		}
		out = append(out, c)
	}
	return writePNGChunks(out), nil
}
