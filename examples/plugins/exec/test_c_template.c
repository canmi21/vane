#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdbool.h>

#define MAX_INPUT 8192

// --- Main Logic ---

int main() {
    char input_raw[MAX_INPUT] = {0};
    size_t pos = 0;

    // Print debug info to Stderr
    fprintf(stderr, "⚙ Starting execution...\n");

    // Read all stdin safely
    int c;
    while ((c = fgetc(stdin)) != EOF && pos < MAX_INPUT - 1) {
        input_raw[pos++] = (char)c;
    }
    input_raw[pos] = '\0';

    if (pos == 0) {
        fprintf(stderr, "✗ No input received on Stdin!\n");
        return 1;
    }

    fprintf(stderr, "⚙ Received Input: %s\n", input_raw);

    // Parse JSON manually (simplified for {"auth_token":"..."} structure)
    char *token_key = "\"auth_token\":\"";
    char *token_start = strstr(input_raw, token_key);
    char auth_token[256] = {0};
    if (token_start) {
        token_start += strlen(token_key);
        char *token_end = strchr(token_start, '"');
        if (token_end) {
            size_t len = token_end - token_start;
            if (len >= sizeof(auth_token)) len = sizeof(auth_token)-1;
            strncpy(auth_token, token_start, len);
            auth_token[len] = '\0';
        }
    }

    // Business Logic
    bool success = (strcmp(auth_token, "secret123") == 0);
    char output[1024];

    if (success) {
        fprintf(stderr, "✓ Auth success!\n");
        snprintf(output, sizeof(output),
                "{\"branch\":\"success\",\"store\":{\"user_role\":\"admin\",\"verified\":\"true\"}}");
    } else {
        fprintf(stderr, "✗ Auth failed. Token was: %s\n", auth_token);
        snprintf(output, sizeof(output),
                "{\"branch\":\"failure\",\"store\":{\"error_reason\":\"invalid_token\"}}");
    }

    // Output result to Stdout (compact JSON)
    fputs(output, stdout);

    return 0;
}
