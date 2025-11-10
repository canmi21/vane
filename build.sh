#!/bin/sh

# Exit immediately if a command exits with a non-zero status.
set -e

# Run the cargo install command.
# The --color=always flag ensures the output is colorized even when piped.
cargo install --color=always --path .