#!/bin/bash

cd ..
# source tests/.venv/bin/activate.fish
ruff format .

cd engine
cargo fmt --all
cd ..

cd console
pnpm lint
cd ..

cd pages
pnpm lint
cd ..