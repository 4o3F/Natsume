#!/bin/bash

# 构建 client（不带 server feature）
echo "Building client..."
cargo build --bin client

# 构建 server（带 server feature）
echo "Building server..."
cargo build --bin server

echo "Builds completed!"
