#!/usr/bin/env node

const readline = require('readline');

// Safely read all content from Stdin
function readAllStdin() {
    return new Promise((resolve) => {
        let lines = [];
        const rl = readline.createInterface({
            input: process.stdin,
            terminal: false
        });

        rl.on('line', (line) => {
            lines.push(line);
        });

        rl.on('close', () => {
            resolve(lines.join('\n'));
        });
    });
}

// --- Main Logic ---

(async () => {
    // Print debug info to Stderr (Vane logs this)
    console.error('⚙ Starting execution...');

    // Read ResolvedInputs from Vane
    const inputRaw = await readAllStdin();
    if (!inputRaw) {
        console.error('✗ No input received on Stdin!');
        process.exit(1);
    }

    console.error('⚙ Received Input: ' + inputRaw);

    // Parse JSON
    let inputs;
    try {
        inputs = JSON.parse(inputRaw);
    } catch (e) {
        console.error('✗ Invalid JSON: ' + e.message);
        process.exit(1);
    }

    // Business Logic
    // Assume Vane passes an argument "auth_token"
    // If token is "secret123", return success branch and write user info to KV
    let output = {};

    if (inputs.auth_token === 'secret123') {
        console.error('✓ Auth success!');
        output = {
            branch: 'success',
            store: {
                user_role: 'admin',
                verified: 'true'
            }
        };
    } else {
        console.error(`✗ Auth failed. Token was: ${inputs.auth_token}`);
        output = {
            branch: 'failure',
            store: {
                error_reason: 'invalid_token'
            }
        };
    }

    // Output result to Stdout (Must be compact JSON)
    process.stdout.write(JSON.stringify(output));
})();
