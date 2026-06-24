/*
 * nucleus.h — C ABI for embedding Nucleus in-process (nucleus.dll / libnucleus.so).
 *
 * Conventions
 * -----------
 * - The engine lives behind an opaque handle (`NucleusEngine*`) from
 *   `nucleus_open`; release it with `nucleus_close`.
 * - Data-carrying calls take a JSON input string and write a JSON output string
 *   through `out_json` (caller frees it with `nucleus_string_free`).
 * - Every call returns an int status: 0 = success, < 0 = failure. On failure,
 *   `*out_json` holds {"error": "..."} and the same message is available from
 *   `nucleus_last_error()` (valid until the next call on the same thread).
 * - Strings are UTF-8, NUL-terminated.
 */
#ifndef NUCLEUS_H
#define NUCLEUS_H

#include <stddef.h> /* size_t */

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque engine handle. */
typedef struct NucleusEngine NucleusEngine;

/* Status codes. */
#define NUCLEUS_OK             0
#define NUCLEUS_ERR_NULL_ARG  -1
#define NUCLEUS_ERR_UTF8      -2
#define NUCLEUS_ERR_JSON      -3
#define NUCLEUS_ERR_ENGINE    -4

/* lifecycle --------------------------------------------------------------- */

/* Open or create a database. config_json e.g.:
 * {"db_path":"data/nucleus.redb","model_cache":"models","index_kind":"flat","gpu":false} */
int  nucleus_open(const char *config_json, NucleusEngine **out_handle);
void nucleus_close(NucleusEngine *handle);

/* Free a string returned through any out-parameter. NULL-safe. */
void nucleus_string_free(char *s);

/* Last error on the calling thread, or NULL. Valid until the next call. */
const char *nucleus_last_error(void);

/* write ------------------------------------------------------------------- */

/* {"name":"legal","model":null} -> Domain */
int nucleus_create_domain(NucleusEngine *handle, const char *input_json, char **out_json);

/* {"domain_id":N,"title":"...","text":"...","source":null,"metadata":{},
 *  "labels":[],"subdomain":null} -> {"document_id":N,"chunk_count":M} */
int nucleus_ingest_text(NucleusEngine *handle, const char *input_json, char **out_json);

/* Ingest a raw file; the engine extracts text by format (pdf/docx/xlsx/html/md/txt).
 * Metadata in input_json: {"domain_id":N,"filename":"x.pdf","title":null,"labels":[],
 * "subdomain":null}; the bytes are passed separately.
 * -> {"document_id":N,"chunk_count":M,"chars":C} */
int nucleus_ingest_file(NucleusEngine *handle, const char *input_json,
                        const unsigned char *bytes, size_t bytes_len, char **out_json);

/* {"document_id":N} -> {"deleted":true} */
int nucleus_delete_document(NucleusEngine *handle, const char *input_json, char **out_json);

/* edit / delete (cascade) ------------------------------------------------- */

/* {"domain_id":N,"name":"..."} -> Domain */
int nucleus_rename_domain(NucleusEngine *handle, const char *input_json, char **out_json);
/* {"domain_id":N} -> {"deleted":true}  (cascades subdomains/docs/chunks/tags) */
int nucleus_delete_domain(NucleusEngine *handle, const char *input_json, char **out_json);
/* {"subdomain_id":N} -> {"deleted":true}  (cascades its documents) */
int nucleus_delete_subdomain(NucleusEngine *handle, const char *input_json, char **out_json);
/* {"tag_id":N,"display_name":"...","description":"..."} -> Tag */
int nucleus_update_tag(NucleusEngine *handle, const char *input_json, char **out_json);
/* {"tag_id":N} -> {"deleted":true}  (detaches; docs/chunks survive) */
int nucleus_delete_tag(NucleusEngine *handle, const char *input_json, char **out_json);
/* {"document_id":N,"labels":["a"],"subdomain":"x","clear_subdomain":false} -> Document */
int nucleus_update_document(NucleusEngine *handle, const char *input_json, char **out_json);
/* {"domain_id":N,"model":null} -> {"reindexed":N}  (re-embed + rebuild index) */
int nucleus_reindex_domain(NucleusEngine *handle, const char *input_json, char **out_json);

/* search ------------------------------------------------------------------ */

/* {"domain_id":N,"query":"...","k":10,"labels":[],"match_all":false,
 *  "document_ids":[],"subdomain":null,"filter":null,"diversity":0.0}
 *  -> {"hits":[{"chunk":{...},"score":0.87,"snippet":"..."}]} */
int nucleus_search(NucleusEngine *handle, const char *input_json, char **out_json);

/* {"domain_ids":[1,2],"query":"...","k":10,"filter":null,"diversity":0.0}
 *  -> {"hits":[...]}  (domains must share a model) */
int nucleus_search_multi(NucleusEngine *handle, const char *input_json, char **out_json);

/* read / browse ----------------------------------------------------------- */

int nucleus_list_domains(NucleusEngine *handle, char **out_json);                     /* {"domains":[...]} */
int nucleus_list_tags(NucleusEngine *handle, const char *input_json, char **out_json);       /* {"domain_id":N} -> {"tags":[...]} */
int nucleus_list_subdomains(NucleusEngine *handle, const char *input_json, char **out_json); /* {"domain_id":N} -> {"subdomains":[...]} */
int nucleus_list_documents(NucleusEngine *handle, const char *input_json, char **out_json);  /* {"domain_id":N,"offset":0,"limit":100} -> {"documents":[...]} */
int nucleus_get_document(NucleusEngine *handle, const char *input_json, char **out_json);    /* {"document_id":N} -> Document */
int nucleus_chunk_context(NucleusEngine *handle, const char *input_json, char **out_json);   /* {"chunk_id":N,"before":1,"after":1} -> {"chunks":[...]} */

/* maintenance ------------------------------------------------------------- */

int nucleus_persist_indexes(NucleusEngine *handle, char **out_json);                  /* {"persisted":N} */

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* NUCLEUS_H */
