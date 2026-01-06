/* examples/plugins/exec/test_cpp_template.cpp */

#include <iostream>
#include <string>

int main() {
    std::cerr << "⚙ Starting execution..." << std::endl;

    // Read all stdin
    std::string input_raw, line;
    while (std::getline(std::cin, line)) {
        input_raw += line + "\n";
    }

    if (input_raw.empty()) {
        std::cerr << "✗ No input received on Stdin!" << std::endl;
        return 1;
    }

    // Remove trailing newline
    if (!input_raw.empty() && input_raw.back() == '\n')
        input_raw.pop_back();

    std::cerr << "⚙ Received Input: " << input_raw << std::endl;

    // Parse JSON manually
    std::string auth_token;
    size_t pos = input_raw.find("\"auth_token\":\"");
    if (pos != std::string::npos) {
        pos += 14;
        size_t end = input_raw.find('"', pos);
        if (end != std::string::npos) {
            auth_token = input_raw.substr(pos, end - pos);
        }
    }

    std::string branch, store;
    if (auth_token == "secret123") {
        std::cerr << "✓ Auth success!" << std::endl;
        branch = "success";
        store = "{\"user_role\":\"admin\",\"verified\":\"true\"}";
    } else {
        std::cerr << "✗ Auth failed. Token was: " << auth_token << std::endl;
        branch = "failure";
        store = "{\"error_reason\":\"invalid_token\"}";
    }

    // Output result
    std::cout << "{\"branch\":\"" << branch << "\",\"store\":" << store << "}";
}