/* Minimal C embedder smoke test for the c2bf C ABI.
 *
 * Build (from the repo root, after building the shared library):
 *   cc -std=c11 -Wall -Wextra -Werror rs/ffi/example.c -Irs/ffi \
 *     -o /tmp/c2bf_example -Lrs/target/release -lc2bf
 * Run with the library on the loader path:
 *   LD_LIBRARY_PATH=rs/target/release /tmp/c2bf_example       (Linux)
 *   DYLD_LIBRARY_PATH=rs/target/release /tmp/c2bf_example   (macOS)
 * Exit code is 0 only when compile+run, format, and the error path
 * all behave as contracted.
 */
#include "c2bf.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int fail(const char *what) {
    fprintf(stderr, "example: %s: %s\n", what,
            c2bf_last_error() ? c2bf_last_error() : "(no error)");
    return 1;
}

int main(void) {
    printf("c2bf version %s\n", c2bf_version());

    /* Compile, then run: 6 * 7 must print "42\n". */
    const char *src = "int main() { printf(\"%d\\n\", 6 * 7); }\n";
    uint8_t *bf = NULL;
    size_t bf_len = 0;
    if (c2bf_compile((const uint8_t *)src, strlen(src), &bf, &bf_len) != 0) {
        return fail("compile");
    }
    uint8_t *out = NULL;
    size_t out_len = 0;
    if (c2bf_run(bf, bf_len, NULL, 0, &out, &out_len) != 0) {
        c2bf_free(bf, bf_len);
        return fail("run");
    }
    if (out_len != 3 || memcmp(out, "42\n", 3) != 0) {
        fprintf(stderr, "example: unexpected output (%zu bytes)\n", out_len);
        c2bf_free(bf, bf_len);
        c2bf_free(out, out_len);
        return 1;
    }
    fwrite(out, 1, out_len, stdout);
    c2bf_free(bf, bf_len);
    c2bf_free(out, out_len);

    /* Format: minify keeps only the eight commands plus newline. */
    const char *code = "a + b [ignored]";
    uint8_t *fmt = NULL;
    size_t fmt_len = 0;
    if (c2bf_format((const uint8_t *)code, strlen(code), 1, &fmt, &fmt_len) != 0) {
        return fail("format");
    }
    if (fmt_len != 4 || memcmp(fmt, "+[]\n", 4) != 0) {
        fprintf(stderr, "example: unexpected format output\n");
        c2bf_free(fmt, fmt_len);
        return 1;
    }
    c2bf_free(fmt, fmt_len);

    /* Error path: invalid source must fail with a message. */
    const char *bad = "int main( {";
    uint8_t *err_out = NULL;
    size_t err_len = 0;
    if (c2bf_compile((const uint8_t *)bad, strlen(bad), &err_out, &err_len) == 0) {
        fprintf(stderr, "example: invalid source compiled\n");
        c2bf_free(err_out, err_len);
        return 1;
    }
    if (err_out != NULL || err_len != 0 || c2bf_last_error() == NULL) {
        fprintf(stderr, "example: error path broke the output contract\n");
        return 1;
    }
    fprintf(stderr, "expected error: %s\n", c2bf_last_error());

    printf("example ok\n");
    return 0;
}
