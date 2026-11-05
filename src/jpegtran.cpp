/*
 * jpegtran.c
 *
 * This file was part of the Independent JPEG Group's software:
 * Copyright (C) 1995-2010, Thomas G. Lane, Guido Vollbeding.
 * libjpeg-turbo Modifications:
 * Copyright (C) 2010, 2014, D. R. Commander.
 * mozjpeg Modifications:
 * Copyright (C) 2014, Mozilla Corporation.
 * For conditions of distribution and use, see the accompanying README file.
 *
 * This file contains a command-line user interface for JPEG transcoding.
 * It is very similar to cjpeg.c, and partly to djpeg.c, but provides
 * lossless transcoding between different JPEG file formats.  It also
 * provides some lossless and sort-of-lossless transformations of JPEG data.
 */

/* Modified by Felix Hanau. */

#include "mozjpeg/jinclude.h"
#include "mozjpeg/jpeglib.h"
#include "mozjpeg/transupp.h"           /* Support routines for jpegtran */
#include "main.h"
#include "support.h"
#include <csetjmp>
#include <cstring>
#include <vector>

/* libjpeg's default error handler calls exit(), which used to end the whole
   ECT run (all remaining files included) on the first damaged JPEG. This
   handler jumps back into mozjpegtran instead, and it counts warnings that
   mean the input is damaged, so that such files are left untouched. */
struct ECTJpegErrorMgr {
  struct jpeg_error_mgr pub;
  jmp_buf* jump;
  int corrupt_warnings;
};

METHODDEF(void)
ect_error_exit (j_common_ptr cinfo)
{
  (*cinfo->err->output_message) (cinfo);
  longjmp(*reinterpret_cast<ECTJpegErrorMgr*>(cinfo->err)->jump, 1);
}

METHODDEF(void)
ect_emit_message (j_common_ptr cinfo, int msg_level)
{
  if (msg_level >= 0) {
    return;  /* trace messages */
  }
  /* Every warning means damaged input: premature end of data, corrupt
     Huffman data, ... make libjpeg invent coefficients, and writing them
     back would replace the damaged original with a "valid" but different
     image. That includes JWRN_EXTRANEOUS_DATA ("N extraneous bytes before
     marker"): it is also what is left of a scan whose entropy-coded data
     was cut or mangled, and in a progressive file the decoded image then
     changes once the scans are rewritten. (0xFF fill bytes, which the
     standard allows, do not raise it.) */
  reinterpret_cast<ECTJpegErrorMgr*>(cinfo->err)->corrupt_warnings++;
  if (cinfo->err->num_warnings == 0) {
    (*cinfo->err->output_message) (cinfo);
  }
  cinfo->err->num_warnings++;
}

/* Writes a whole file from two consecutive pieces (the second may be empty).
   Returns 0 on success, 1 if the file could not be opened (it is untouched)
   and 2 if writing failed after truncating it. */
static int write_whole_file (const char * path, const unsigned char * data, size_t size,
                             const unsigned char * data2, size_t size2)
{
  FILE * f = fopen(path, "wb");
  if (!f) {
    return 1;
  }
  bool ok = fwrite(data, 1, size, f) == size;
  if (ok && size2) {
    ok = fwrite(data2, 1, size2, f) == size2;
  }
  ok = fclose(f) == 0 && ok;
  return ok ? 0 : 2;
}

static size_t jcopy_markers_execute_s (j_decompress_ptr srcinfo, j_compress_ptr dstinfo)
{
  size_t size = 0;
  for (jpeg_saved_marker_ptr marker = srcinfo->marker_list; marker; marker = marker->next) {
    if (dstinfo->write_JFIF_header &&
        marker->marker == JPEG_APP0 &&
        marker->data_length >= 5 &&
        GETJOCTET(marker->data[0]) == 0x4A &&
        GETJOCTET(marker->data[1]) == 0x46 &&
        GETJOCTET(marker->data[2]) == 0x49 &&
        GETJOCTET(marker->data[3]) == 0x46 &&
        GETJOCTET(marker->data[4]) == 0)
      continue;                 /* reject duplicate JFIF */
    if (dstinfo->write_Adobe_marker &&
        marker->marker == JPEG_APP0+14 &&
        marker->data_length >= 5 &&
        GETJOCTET(marker->data[0]) == 0x41 &&
        GETJOCTET(marker->data[1]) == 0x64 &&
        GETJOCTET(marker->data[2]) == 0x6F &&
        GETJOCTET(marker->data[3]) == 0x62 &&
        GETJOCTET(marker->data[4]) == 0x65)
      continue;                 /* reject duplicate Adobe */
    jpeg_write_marker(dstinfo, marker->marker, marker->data, marker->data_length);
    if(marker->marker == JPEG_COM || (marker->marker >= JPEG_APP0 && marker->marker <= JPEG_APP0 + 15)){
      size += marker->data_length;
    }
  }
  return size;
}

/* Map Exif orientation values to correct JXFORM_CODE */
static JXFORM_CODE orient_jxform[9] = {
  JXFORM_NONE,
  JXFORM_NONE,
  JXFORM_FLIP_H,
  JXFORM_ROT_180,
  JXFORM_FLIP_V,
  JXFORM_TRANSPOSE,
  JXFORM_ROT_90,
  JXFORM_TRANSVERSE,
  JXFORM_ROT_270
};

/* Get Exif image orientation. Copied from adjust_exif_parameters. */
LOCAL(unsigned int)
get_exif_orientation (JOCTET *data, unsigned int length)
{
  boolean is_motorola; /* Flag for byte order */
  unsigned int number_of_tags, tagnum;
  unsigned int offset;

  if (length < 12) return 0; /* Length of an IFD entry */

  /* Discover byte order */
  if (GETJOCTET(data[0]) == 0x49 && GETJOCTET(data[1]) == 0x49)
    is_motorola = FALSE;
  else if (GETJOCTET(data[0]) == 0x4D && GETJOCTET(data[1]) == 0x4D)
    is_motorola = TRUE;
  else
    return 0;

  /* Check Tag Mark */
  if (is_motorola) {
    if (GETJOCTET(data[2]) != 0) return 0;
    if (GETJOCTET(data[3]) != 0x2A) return 0;
  } else {
    if (GETJOCTET(data[3]) != 0) return 0;
    if (GETJOCTET(data[2]) != 0x2A) return 0;
  }

  /* Get first IFD offset (offset to IFD0) */
  if (is_motorola) {
    if (GETJOCTET(data[4]) != 0) return 0;
    if (GETJOCTET(data[5]) != 0) return 0;
    offset = GETJOCTET(data[6]);
    offset <<= 8;
    offset += GETJOCTET(data[7]);
  } else {
    if (GETJOCTET(data[7]) != 0) return 0;
    if (GETJOCTET(data[6]) != 0) return 0;
    offset = GETJOCTET(data[5]);
    offset <<= 8;
    offset += GETJOCTET(data[4]);
  }
  if (offset > length - 2) return 0; /* check end of data segment */

  /* Get the number of directory entries contained in this IFD */
  if (is_motorola) {
    number_of_tags = GETJOCTET(data[offset]);
    number_of_tags <<= 8;
    number_of_tags += GETJOCTET(data[offset+1]);
  } else {
    number_of_tags = GETJOCTET(data[offset+1]);
    number_of_tags <<= 8;
    number_of_tags += GETJOCTET(data[offset]);
  }
  if (number_of_tags == 0) return 0;
  offset += 2;

  /* Search for Orientation Tag in this IFD */
  do {
    if (offset > length - 12) return 0; /* check end of data segment */
    /* Get Tag number */
    if (is_motorola) {
      tagnum = GETJOCTET(data[offset]);
      tagnum <<= 8;
      tagnum += GETJOCTET(data[offset+1]);
    } else {
      tagnum = GETJOCTET(data[offset+1]);
      tagnum <<= 8;
      tagnum += GETJOCTET(data[offset]);
    }
    if (tagnum == 0x0112) {
      if (is_motorola) {
        return GETJOCTET(data[offset+9]);
      } else {
        return GETJOCTET(data[offset+8]);
      }
    }
    offset += 12;
  } while (--number_of_tags);
  return 0;
}

METHODDEF(void)
output_message (j_common_ptr cinfo)
{
  char buffer[JMSG_LENGTH_MAX];

  /* Create the message */
  (*cinfo->err->format_message) (cinfo, buffer);

  /* Send it to stderr, adding a newline */
  fprintf(stderr, "%s: %s\n", cinfo->err->addon_message_table[0], buffer);
}

int mozjpegtran (bool arithmetic, bool progressive, bool strip, unsigned autorotate, const char * Infile, const char * Outfile, size_t* stripped_outsize)
{
  struct jpeg_decompress_struct srcinfo;
  struct jpeg_compress_struct dstinfo;
  ECTJpegErrorMgr jsrcerr, jdsterr;
  jmp_buf jump;
  jpeg_transform_info transformoption; /* image transformation options */
  FILE * fp;
  unsigned char *outbuffer = 0;
  unsigned long outsize = 0;
  size_t extrasize = 0;
  unsigned char copy_exif = 0;
  const char* addon = Infile;
  /* Initialize the JPEG decompression and compression objects with error
     handlers that return here instead of terminating the process. */
  for (ECTJpegErrorMgr* e : {&jsrcerr, &jdsterr}) {
    jpeg_std_error(&e->pub);
    e->pub.error_exit = ect_error_exit;
    e->pub.emit_message = ect_emit_message;
    e->pub.output_message = output_message;
    e->pub.addon_message_table = &addon;
    e->jump = &jump;
    e->corrupt_warnings = 0;
  }
  srcinfo.err = &jsrcerr.pub;
  jpeg_create_decompress(&srcinfo);
  dstinfo.err = &jdsterr.pub;
  jpeg_create_compress(&dstinfo);
  auto cleanup = [&srcinfo, &dstinfo]() {
    jpeg_destroy_compress(&dstinfo);
    jpeg_destroy_decompress(&srcinfo);
  };
  if (!progressive){
    jpeg_c_set_int_param(&dstinfo, JINT_COMPRESS_PROFILE, JCP_FASTEST);
  }

  /* Open the input file. */
  if (!(fp = fopen(Infile, "rb"))) {
    fprintf(stderr, "ECT: can't open %s for reading\n", Infile);
    cleanup();
    return 2;
  }

  long long insize = filesize(Infile);
  if(insize < 0){
    fprintf(stderr, "ECT: can't read from %s\n", Infile);
    fclose(fp);
    cleanup();
    return 2;
  }
  std::vector<unsigned char> inbuffer((size_t)insize);

  if (fread(inbuffer.data(), 1, (size_t)insize, fp) != (size_t)insize) {
    fprintf(stderr, "ECT: can't read from %s\n", Infile);
    fclose(fp);
    cleanup();
    return 2;
  }
  fclose(fp);

  /* Fatal libjpeg errors (invalid or unsupported data) land here; the
     message has already been printed by ect_error_exit. jpeg_mem_dest keeps
     outbuffer pointing at its current buffer, so it can be freed here. This
     must be set up before jpeg_mem_src, which already fails on empty
     input. */
  if (setjmp(jump)) {
    free(outbuffer);
    cleanup();
    return 2;
  }

  jpeg_mem_src(&srcinfo, inbuffer.data(), (size_t)insize);

  /* Enable saving of extra markers that we want to copy */
  if (!strip) {
    jcopy_markers_setup(&srcinfo, JCOPYOPT_ALL);
  } else if (autorotate > 0) {
    jpeg_save_markers(&srcinfo, JPEG_APP0+1, 0xFFFF);
  }

  /* Read file header */
  jpeg_read_header(&srcinfo, 1);

  /* Determine orientation from Exif */
  transformoption.transform = JXFORM_NONE;
  if (autorotate > 0 && srcinfo.marker_list != NULL &&
      srcinfo.marker_list->marker == JPEG_APP0+1 &&
      srcinfo.marker_list->data_length >= 6 &&
      GETJOCTET(srcinfo.marker_list->data[0]) == 0x45 &&
      GETJOCTET(srcinfo.marker_list->data[1]) == 0x78 &&
      GETJOCTET(srcinfo.marker_list->data[2]) == 0x69 &&
      GETJOCTET(srcinfo.marker_list->data[3]) == 0x66 &&
      GETJOCTET(srcinfo.marker_list->data[4]) == 0 &&
      GETJOCTET(srcinfo.marker_list->data[5]) == 0) {
    unsigned int orientation = get_exif_orientation(srcinfo.marker_list->data + 6,
                                                    srcinfo.marker_list->data_length - 6);
    /* Setup transform options for auto-rotate */
    if (orientation > 1 && orientation <= 8) {
      transformoption.transform = orient_jxform[orientation];
      transformoption.perfect = autorotate > 1;
      transformoption.trim = TRUE;
      transformoption.force_grayscale = FALSE;
      transformoption.crop = FALSE;
      transformoption.slow_hflip = FALSE;
      /* If perfect requested but not possible, show warning and do not transform */
      if (!jtransform_request_workspace(&srcinfo, &transformoption)) {
        fprintf(stderr, "ECT: %s can't be transformed perfectly\n", Infile);
        transformoption.transform = JXFORM_NONE;
        copy_exif = 1;
      }
    }
  }

  /* Read source file as DCT coefficients */
  jvirt_barray_ptr * src_coef_arrays = jpeg_read_coefficients(&srcinfo);
  if (jsrcerr.corrupt_warnings) {
    fprintf(stderr, "ECT: %s: corrupt JPEG data, file left unchanged\n", Infile);
    cleanup();
    return 2;
  }
  /* Look for an MPF (multi-picture) APP2 segment now: jpeg_finish_decompress
     releases the saved markers. */
  bool has_mpf = false;
  for (jpeg_saved_marker_ptr m = srcinfo.marker_list; m; m = m->next) {
    if (m->marker == JPEG_APP0 + 2 && m->data_length >= 4 && memcmp(m->data, "MPF\0", 4) == 0) {
      has_mpf = true;
    }
  }

  /* Initialize destination compression parameters from source values */
  jpeg_copy_critical_parameters(&srcinfo, &dstinfo);

  /* Adjust destination parameters if required by transform options;
   * also find out which set of coefficient arrays will hold the output.
   */
  jvirt_barray_ptr * dst_coef_arrays = src_coef_arrays;
  if (transformoption.transform != JXFORM_NONE) {
    dst_coef_arrays = jtransform_adjust_parameters(&srcinfo, &dstinfo,
                                                   src_coef_arrays,
                                                   &transformoption);
  }

  /* Adjust default compression parameters by re-parsing the options */
  dstinfo.optimize_coding = !arithmetic;
  dstinfo.arith_code = arithmetic;
  if (!dstinfo.num_scans || !progressive) {
    dstinfo.num_scans = 0;
    dstinfo.scan_info = 0;
  }

  /* Specify data destination for compression */
  jpeg_mem_dest(&dstinfo, &outbuffer, &outsize);

  /* Start compressor (note no image data is actually written here) */
  jpeg_write_coefficients(&dstinfo, dst_coef_arrays);

  /* Copy to the output file any extra markers that we want to preserve */
  if (!strip || copy_exif) {
    extrasize = jcopy_markers_execute_s(&srcinfo, &dstinfo);
  }

  /* Execute image transformation, if any */
  if (transformoption.transform != JXFORM_NONE) {
    jtransform_execute_transformation(&srcinfo, &dstinfo,
                                      src_coef_arrays,
                                      &transformoption);
  }

  /* Finish compression and release memory */
  jpeg_finish_compress(&dstinfo);
  /* Read the rest of the input up to EOI before deciding to write: damage
     found there must also keep the original file. */
  jpeg_finish_decompress(&srcinfo);
  if (jsrcerr.corrupt_warnings) {
    fprintf(stderr, "ECT: %s: corrupt JPEG data, file left unchanged\n", Infile);
    free(outbuffer);
    cleanup();
    return 2;
  }

  /* Data after the EOI marker is not part of the image, but it often matters:
     phones append the video of "motion photos" there, and multi-picture
     files (MPF/MPO) store further images there. It is kept unless metadata
     is stripped. MPF locates its images by offsets that would change with
     the first image's size, so such files are left untouched. */
  const unsigned char* trailing = srcinfo.src->next_input_byte;
  size_t trailing_size = strip ? 0 : srcinfo.src->bytes_in_buffer;
  if (trailing_size && has_mpf) {
    fprintf(stderr, "ECT: %s: multi-picture JPEG (MPF) is not supported, file left unchanged\n", Infile);
    free(outbuffer);
    cleanup();
    return 1;
  }
  const unsigned long long total = (unsigned long long)outsize + trailing_size;

  int result = (unsigned long long)insize < total;

  if (total < (unsigned long long)insize){
    /* Rewrite the file in place (keeping its permissions, owner and hard
       links). If writing fails after the file was truncated, put the
       original bytes back. */
    int err = write_whole_file(Outfile, outbuffer, outsize, trailing, trailing_size);
    if (err) {
      fprintf(stderr, "ECT: can't write to %s\n", Outfile);
      if (err == 2 && write_whole_file(Outfile, inbuffer.data(), inbuffer.size(), 0, 0)) {
        fprintf(stderr, "ECT: could not restore the original contents of %s\n", Outfile);
      }
      result = 2;
    }
  }

  free(outbuffer);
  cleanup();
  (*stripped_outsize) = outsize - extrasize;
  return result;
}
