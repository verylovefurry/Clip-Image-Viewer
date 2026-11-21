// Minimal C ABI over LibRaw for the isolated RAW worker: embedded thumbnail or a developed
// 8-bit sRGB image. Each call uses its own LibRaw instance; nothing is shared between calls.
#include "libraw/libraw.h"

#include <new>

extern "C" {

struct ClipViewRawImage {
  int kind;        // 1 = JPEG bytes, 2 = packed RGB8
  int width;
  int height;
  int flip;        // LibRaw orientation: 0, 3 (180), 5 (90 CCW), 6 (90 CW); developed images are already rotated
  unsigned long long size;
  const unsigned char *data;
  void *owner;
};

static void reset(ClipViewRawImage *out) {
  out->kind = 0; out->width = 0; out->height = 0; out->flip = 0;
  out->size = 0; out->data = nullptr; out->owner = nullptr;
}

static int finish(LibRaw *raw, libraw_processed_image_t *image, ClipViewRawImage *out) {
  if (image->type == LIBRAW_IMAGE_JPEG) {
    out->kind = 1;
  } else if (image->type == LIBRAW_IMAGE_BITMAP && image->colors == 3 && image->bits == 8) {
    out->kind = 2;
  } else {
    LibRaw::dcraw_clear_mem(image);
    return LIBRAW_UNSUPPORTED_THUMBNAIL;
  }
  out->width = image->width;
  out->height = image->height;
  out->flip = raw->imgdata.sizes.flip;
  out->size = image->data_size;
  out->data = image->data;
  out->owner = image;
  return LIBRAW_SUCCESS;
}

static LibRaw *open_raw(const unsigned char *data, size_t len, unsigned max_raw_mb, int *error) {
  LibRaw *raw = new (std::nothrow) LibRaw(0);
  if (!raw) { *error = LIBRAW_UNSUFFICIENT_MEMORY; return nullptr; }
  raw->imgdata.rawparams.max_raw_memory_mb = max_raw_mb;
  *error = raw->open_buffer(data, len);
  if (*error != LIBRAW_SUCCESS) { delete raw; return nullptr; }
  return raw;
}

int clipview_libraw_thumbnail(const unsigned char *data, size_t len, unsigned max_raw_mb, ClipViewRawImage *out) {
  reset(out);
  try {
    int error = 0;
    LibRaw *raw = open_raw(data, len, max_raw_mb, &error);
    if (!raw) return error;
    error = raw->unpack_thumb();
    if (error == LIBRAW_SUCCESS) {
      libraw_processed_image_t *image = raw->dcraw_make_mem_thumb(&error);
      error = image ? finish(raw, image, out) : (error ? error : LIBRAW_UNSUPPORTED_THUMBNAIL);
    }
    delete raw;
    return error;
  } catch (...) {
    return LIBRAW_UNSPECIFIED_ERROR;
  }
}

int clipview_libraw_develop(const unsigned char *data, size_t len, unsigned max_raw_mb, int half_size, ClipViewRawImage *out) {
  reset(out);
  try {
    int error = 0;
    LibRaw *raw = open_raw(data, len, max_raw_mb, &error);
    if (!raw) return error;
    raw->imgdata.params.half_size = half_size;
    raw->imgdata.params.use_camera_wb = 1;
    raw->imgdata.params.output_bps = 8;
    raw->imgdata.params.user_qual = 0;  // bilinear: fast, adequate for a viewer preview
    error = raw->unpack();
    if (error == LIBRAW_SUCCESS) error = raw->dcraw_process();
    if (error == LIBRAW_SUCCESS) {
      libraw_processed_image_t *image = raw->dcraw_make_mem_image(&error);
      error = image ? finish(raw, image, out) : (error ? error : LIBRAW_UNSPECIFIED_ERROR);
      out->flip = 0;
    }
    delete raw;
    return error;
  } catch (...) {
    return LIBRAW_UNSPECIFIED_ERROR;
  }
}

int clipview_libraw_raw_pixels(const unsigned char *data, size_t len, unsigned max_raw_mb, unsigned long long *pixels) {
  *pixels = 0;
  try {
    int error = 0;
    LibRaw *raw = open_raw(data, len, max_raw_mb, &error);
    if (!raw) return error;
    *pixels = (unsigned long long)raw->imgdata.sizes.width * raw->imgdata.sizes.height;
    delete raw;
    return LIBRAW_SUCCESS;
  } catch (...) {
    return LIBRAW_UNSPECIFIED_ERROR;
  }
}

void clipview_libraw_free(ClipViewRawImage *image) {
  if (image->owner) LibRaw::dcraw_clear_mem(static_cast<libraw_processed_image_t *>(image->owner));
  reset(image);
}

}
