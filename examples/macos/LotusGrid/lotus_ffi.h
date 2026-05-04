#ifndef LOTUS_FFI_H
#define LOTUS_FFI_H

#ifdef __cplusplus
extern "C" {
#endif

typedef struct LotusSheet LotusSheet;

LotusSheet* lotus_sheet_new(void);
void        lotus_sheet_free(LotusSheet* sheet);

/* Returns NULL on success, owned error string on failure. Free with lotus_string_free. */
char* lotus_sheet_set(LotusSheet* sheet, const char* cell, const char* value);

/* Both return owned strings; always free with lotus_string_free. */
char* lotus_sheet_get(const LotusSheet* sheet, const char* cell);
char* lotus_sheet_get_raw(const LotusSheet* sheet, const char* cell);

void lotus_string_free(char* s);

#ifdef __cplusplus
}
#endif

#endif
