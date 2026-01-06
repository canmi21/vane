// examples/plugins/exec/test_java_template.java

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.io.IOException;

class test_java_template {
    public static void main(String[] args) throws IOException {
        // Print debug info to Stderr
        System.err.println("⚙ Starting execution...");

        // Read all stdin safely
        BufferedReader reader = new BufferedReader(new InputStreamReader(System.in));
        StringBuilder inputRaw = new StringBuilder();
        String line;
        while ((line = reader.readLine()) != null) {
            inputRaw.append(line).append("\n");
        }

        if (inputRaw.length() == 0) {
            System.err.println("✗ No input received on Stdin!");
            System.exit(1);
        }

        // Remove trailing newline
        if (inputRaw.charAt(inputRaw.length() - 1) == '\n') {
            inputRaw.deleteCharAt(inputRaw.length() - 1);
        }

        System.err.println("⚙ Received Input: " + inputRaw);

        // Parse JSON manually for {"auth_token":"..."} structure
        String inputStr = inputRaw.toString();
        String authToken = "";
        String key = "\"auth_token\":\"";
        int idx = inputStr.indexOf(key);
        if (idx != -1) {
            int start = idx + key.length();
            int end = inputStr.indexOf('"', start);
            if (end != -1) {
                authToken = inputStr.substring(start, end);
            }
        }

        // Business Logic
        String branch;
        String store;
        if ("secret123".equals(authToken)) {
            System.err.println("✓ Auth success!");
            branch = "success";
            store = "{\"user_role\":\"admin\",\"verified\":\"true\"}";
        } else {
            System.err.println("✗ Auth failed. Token was: " + authToken);
            branch = "failure";
            store = "{\"error_reason\":\"invalid_token\"}";
        }

        // Output result to Stdout (compact JSON)
        System.out.print("{\"branch\":\"" + branch + "\",\"store\":" + store + "}");
    }
}