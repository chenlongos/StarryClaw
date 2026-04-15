#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <netdb.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/types.h>
#include <unistd.h>

static char *sc_dup_str(const char *s) {
    size_t n = strlen(s) + 1;
    char *p = (char *)malloc(n);
    if (!p) return NULL;
    memcpy(p, s, n);
    return p;
}

static int sc_connect_tcp(const char *host, int port, int timeout_secs, int *out_fd, char **err) {
    struct addrinfo hints;
    struct addrinfo *res = NULL, *rp = NULL;
    char port_str[16];
    int fd = -1;
    int rc;

    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    snprintf(port_str, sizeof(port_str), "%d", port);

    rc = getaddrinfo(host, port_str, &hints, &res);
    if (rc != 0) {
        char msg[256];
        snprintf(msg, sizeof(msg), "getaddrinfo failed: %s", gai_strerror(rc));
        *err = sc_dup_str(msg);
        return -1;
    }

    for (rp = res; rp; rp = rp->ai_next) {
        fd = socket(rp->ai_family, rp->ai_socktype, rp->ai_protocol);
        if (fd < 0) continue;

        if (timeout_secs > 0) {
            struct timeval tv;
            tv.tv_sec = timeout_secs;
            tv.tv_usec = 0;
            setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
            setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
        }

        if (connect(fd, rp->ai_addr, rp->ai_addrlen) == 0) {
            break;
        }
        close(fd);
        fd = -1;
    }
    freeaddrinfo(res);

    if (fd < 0) {
        char msg[256];
        snprintf(msg, sizeof(msg), "connect failed: %s", strerror(errno));
        *err = sc_dup_str(msg);
        return -1;
    }
    *out_fd = fd;
    return 0;
}

static int sc_send_all(int fd, const char *buf, size_t len, char **err) {
    size_t sent = 0;
    while (sent < len) {
        ssize_t n = send(fd, buf + sent, len - sent, 0);
        if (n < 0) {
            char msg[256];
            snprintf(msg, sizeof(msg), "send failed: %s", strerror(errno));
            *err = sc_dup_str(msg);
            return -1;
        }
        if (n == 0) {
            *err = sc_dup_str("send returned 0");
            return -1;
        }
        sent += (size_t)n;
    }
    return 0;
}

static int sc_recv_all(int fd, char **out, size_t *out_len, char **err) {
    size_t cap = 64 * 1024;
    size_t len = 0;
    char *buf = (char *)malloc(cap);
    if (!buf) {
        *err = sc_dup_str("malloc failed");
        return -1;
    }

    while (1) {
        if (len + 4096 + 1 > cap) {
            size_t new_cap = cap * 2;
            char *nb = (char *)realloc(buf, new_cap);
            if (!nb) {
                free(buf);
                *err = sc_dup_str("realloc failed");
                return -1;
            }
            buf = nb;
            cap = new_cap;
        }
        ssize_t n = recv(fd, buf + len, 4096, 0);
        if (n < 0) {
            char msg[256];
            snprintf(msg, sizeof(msg), "recv failed: %s", strerror(errno));
            free(buf);
            *err = sc_dup_str(msg);
            return -1;
        }
        if (n == 0) break;
        len += (size_t)n;
    }

    buf[len] = '\0';
    *out = buf;
    *out_len = len;
    return 0;
}

static int sc_headers_have_chunked(const char *headers, size_t header_len) {
    char *tmp = (char *)malloc(header_len + 1);
    if (!tmp) return 0;
    for (size_t i = 0; i < header_len; i++) {
        char c = headers[i];
        if (c >= 'A' && c <= 'Z') c = (char)(c - 'A' + 'a');
        tmp[i] = c;
    }
    tmp[header_len] = '\0';

    int ok = 0;
    char *te = strstr(tmp, "transfer-encoding:");
    if (te && strstr(te, "chunked")) ok = 1;
    free(tmp);
    return ok;
}

static int sc_decode_chunked(const char *chunked, char **decoded, char **err) {
    size_t cap = strlen(chunked) + 1;
    char *out = (char *)malloc(cap);
    size_t out_len = 0;
    const char *p = chunked;
    if (!out) {
        *err = sc_dup_str("malloc chunk buffer failed");
        return -1;
    }

    while (1) {
        char *line_end = strstr(p, "\r\n");
        if (!line_end) {
            free(out);
            *err = sc_dup_str("invalid chunked body: missing chunk size line end");
            return -1;
        }

        char size_buf[32];
        size_t line_len = (size_t)(line_end - p);
        if (line_len == 0 || line_len >= sizeof(size_buf)) {
            free(out);
            *err = sc_dup_str("invalid chunk size line");
            return -1;
        }
        memcpy(size_buf, p, line_len);
        size_buf[line_len] = '\0';

        char *semi = strchr(size_buf, ';');
        if (semi) *semi = '\0';

        char *endptr = NULL;
        unsigned long sz = strtoul(size_buf, &endptr, 16);
        if (endptr == size_buf || *endptr != '\0') {
            free(out);
            *err = sc_dup_str("invalid chunk size value");
            return -1;
        }

        p = line_end + 2;
        if (sz == 0) break;

        if (out_len + sz + 1 > cap) {
            size_t new_cap = cap * 2;
            while (new_cap < out_len + sz + 1) new_cap *= 2;
            char *nb = (char *)realloc(out, new_cap);
            if (!nb) {
                free(out);
                *err = sc_dup_str("realloc chunk buffer failed");
                return -1;
            }
            out = nb;
            cap = new_cap;
        }

        memcpy(out + out_len, p, sz);
        out_len += sz;
        p += sz;
        if (strncmp(p, "\r\n", 2) != 0) {
            free(out);
            *err = sc_dup_str("invalid chunked body: missing chunk delimiter");
            return -1;
        }
        p += 2;
    }

    out[out_len] = '\0';
    *decoded = out;
    return 0;
}

static int sc_body_looks_chunked(const char *body) {
    const char *line_end = strstr(body, "\r\n");
    if (!line_end || line_end == body || (line_end - body) > 16) return 0;
    for (const char *p = body; p < line_end; ++p) {
        char c = *p;
        int is_hex = ((c >= '0' && c <= '9') ||
                      (c >= 'a' && c <= 'f') ||
                      (c >= 'A' && c <= 'F') ||
                      c == ';');
        if (!is_hex) return 0;
    }
    return 1;
}

int sc_http_post_json(
    const char *host,
    int port,
    const char *path,
    const char *json_body,
    const char *bearer_token,
    int timeout_secs,
    int *status_code,
    char **response_body,
    char **error_msg
) {
    int fd = -1;
    char *raw = NULL;
    size_t raw_len = 0;
    char *request = NULL;
    size_t req_cap;
    int ret = -1;
    const char *sep;

    if (!host || !path || !json_body || !status_code || !response_body || !error_msg) {
        return -1;
    }
    *status_code = 0;
    *response_body = NULL;
    *error_msg = NULL;

    req_cap = strlen(json_body) + strlen(host) + strlen(path) + 1024 +
              (bearer_token ? strlen(bearer_token) : 0);
    request = (char *)malloc(req_cap);
    if (!request) {
        *error_msg = sc_dup_str("malloc request failed");
        goto done;
    }

    if (bearer_token && bearer_token[0] != '\0') {
        snprintf(
            request, req_cap,
            "POST %s HTTP/1.1\r\n"
            "Host: %s:%d\r\n"
            "Content-Type: application/json\r\n"
            "Authorization: Bearer %s\r\n"
            "Content-Length: %zu\r\n"
            "Connection: close\r\n"
            "\r\n"
            "%s",
            path, host, port, bearer_token, strlen(json_body), json_body
        );
    } else {
        snprintf(
            request, req_cap,
            "POST %s HTTP/1.1\r\n"
            "Host: %s:%d\r\n"
            "Content-Type: application/json\r\n"
            "Content-Length: %zu\r\n"
            "Connection: close\r\n"
            "\r\n"
            "%s",
            path, host, port, strlen(json_body), json_body
        );
    }

    if (sc_connect_tcp(host, port, timeout_secs, &fd, error_msg) != 0) goto done;
    if (sc_send_all(fd, request, strlen(request), error_msg) != 0) goto done;
    if (sc_recv_all(fd, &raw, &raw_len, error_msg) != 0) goto done;

    if (raw_len < 12 || strncmp(raw, "HTTP/", 5) != 0) {
        *error_msg = sc_dup_str("invalid HTTP response");
        goto done;
    }

    {
        char *sp = strchr(raw, ' ');
        if (!sp || !*(sp + 1)) {
            *error_msg = sc_dup_str("cannot parse status code");
            goto done;
        }
        *status_code = atoi(sp + 1);
    }

    sep = strstr(raw, "\r\n\r\n");
    if (!sep) {
        *error_msg = sc_dup_str("HTTP response missing header/body separator");
        goto done;
    }
    {
        size_t header_len = (size_t)(sep - raw);
        sep += 4;
        if (sc_headers_have_chunked(raw, header_len) || sc_body_looks_chunked(sep)) {
            if (sc_decode_chunked(sep, response_body, error_msg) != 0) {
                goto done;
            }
        } else {
            *response_body = sc_dup_str(sep);
            if (!*response_body) {
                *error_msg = sc_dup_str("malloc response failed");
                goto done;
            }
        }
    }

    ret = 0;

done:
    if (fd >= 0) close(fd);
    free(request);
    free(raw);
    return ret;
}

void sc_http_free(void *ptr) {
    free(ptr);
}
