#ifndef MARKITAI_H
#define MARKITAI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Owns len bytes, without a trailing NUL. Do not copy this ownership handle. */
typedef struct MarkitaiBuffer {
    uint8_t *data;
    size_t len;
} MarkitaiBuffer;

uint32_t markitai_abi_version(void);
/* Static UTF-8 string, never free or modify. */
const char *markitai_version(void);
/* Input is borrowed until return. Maximum request size: 64 MiB. */
MarkitaiBuffer markitai_convert_json(const uint8_t *request, size_t len);
/* Clears the buffer, so repeated free of the same struct is safe. */
void markitai_buffer_free(MarkitaiBuffer *buffer);

#ifdef __cplusplus
}
#endif
#endif
