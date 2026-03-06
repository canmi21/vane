/* integration/tests/l7/cgi-bin/redirect_login.c */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main() {
    // 1. Read Environment Variables
    char *method = getenv("REQUEST_METHOD");
    char *content_length_str = getenv("CONTENT_LENGTH");

    // Debug to stderr (visible in Vane logs)
    fprintf(stderr, "DEBUG: method=%s, content_length=%s\n",
            method ? method : "(null)",
            content_length_str ? content_length_str : "(null)");

    // 2. Read POST Body
    char body[4096] = {0};
    int len = 0;
    if (content_length_str != NULL) {
        len = atoi(content_length_str);
        if (len > 0) {
            if (len > sizeof(body) - 1) len = sizeof(body) - 1;
            fread(body, 1, len, stdin);
            fprintf(stderr, "DEBUG: Read %d bytes from stdin\n", len);
        }
    }

    // 3. Check Authentication (simple string match)
    int authenticated = 0;
    if (method && strcmp(method, "POST") == 0 && len > 0) {
        // Check if body contains "username=" and "password="
        if (strstr(body, "username=") != NULL && strstr(body, "password=") != NULL) {
            authenticated = 1;
            fprintf(stderr, "DEBUG: Authentication SUCCESS\n");
        } else {
            fprintf(stderr, "DEBUG: Authentication FAILED - invalid credentials\n");
        }
    } else {
        fprintf(stderr, "DEBUG: Authentication FAILED - not POST or no body\n");
    }

    // 4. Output Response
    if (authenticated) {
        // Return 302 Redirect with Session Cookie
        printf("Status: 302 Found\r\n");
        printf("Set-Cookie: session_id=test_session_12345; path=/; HttpOnly\r\n");
        printf("Location: /dashboard\r\n");
        printf("Content-Type: text/html\r\n");
        printf("\r\n");
        // Optional body for 302
        printf("<html><body>Redirecting...</body></html>\n");
    } else {
        // Return 401 Unauthorized
        printf("Status: 401 Unauthorized\r\n");
        printf("Content-Type: text/plain\r\n");
        printf("\r\n");
        printf("Authentication required\n");
    }

    fflush(stdout);
    return 0;
}
