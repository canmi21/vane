/* integration/tests/l7/cgi-bin/sample_bin.c */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main() {
    // 1. Read Environment Variables
    char *method = getenv("REQUEST_METHOD");
    char *content_length = getenv("CONTENT_LENGTH");
    char *query = getenv("QUERY_STRING");

    // 2. Read Body (if any)
    char body[4096] = {0};
    int len = 0;
    if (content_length != NULL) {
        len = atoi(content_length);
        if (len > 0) {
            if (len > sizeof(body) - 1) len = sizeof(body) - 1;
            fread(body, 1, len, stdin);
        }
    }

    // 3. Output Headers
    // Must end with \r\n\r\n
    printf("Status: 200 OK\r\n");
    printf("Content-Type: text/plain\r\n");
    printf("X-CGI-Test: Vane-C-Bin\r\n");
    printf("\r\n");

    // 4. Output Body
    printf("CGI Output:\n");
    printf("Method: %s\n", method ? method : "(null)");
    printf("Query: %s\n", query ? query : "(null)");
    printf("Body Len: %d\n", len);
    printf("Body Content: %s\n", body);

    return 0;
}