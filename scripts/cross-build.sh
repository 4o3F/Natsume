#!/bin/bash

echo "Building client..."
cross build --bin client --features="client" --target x86_64-unknown-linux-musl

echo "Building server..."
cross build --bin server --features="server" --target x86_64-unknown-linux-musl

echo "Builds completed!"
